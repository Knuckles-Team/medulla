//! Reward decomposition (MEDULLA-DECISION-ENGINE-R751.751).
//!
//! [`decompose`] splits one trajectory reward over the steps that earned it.
//! A step's weight is the progress its value estimate shows, floored by
//! `knob reward.difficulty_floor` and discounted by
//! `knob reward.cache_tier_discount` when a cache tier served it. The step
//! rewards sum exactly to the trajectory reward: each is floored and the last
//! step takes the rounding remainder. Everything is fixed-point integer
//! arithmetic on `Q32`: no float, no clock, no store, so repeated calls for one
//! trajectory return identical bytes.

use medulla_decision_types::decision::numeric::{micros_to_q32, QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::reward::{
    RewardDecompositionRequest, RewardKnobs, StepReward, TrajectoryReward, MAX_HTN_NODES,
    MAX_REWARD_STEPS,
};
use medulla_types::contract::BoundedVec;

const ONE_MICROS: i128 = 1_000_000;

fn q32(value: i128) -> Result<QuantisedValue, String> {
    Ok(QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: i64::try_from(value).map_err(|_| "a reward overflows scale Q32".to_string())?,
    })
}

fn on_q32(value: &QuantisedValue, what: &str) -> Result<i128, String> {
    if value.scale == QuantScaleTag::Q32 {
        Ok(i128::from(value.value))
    } else {
        Err(format!("{what} must be on scale Q32"))
    }
}

/// The integer square root: the largest `r` with `r * r <= value`.
fn isqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2).max(1);
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

/// The step weights, in step order, on `Q32`.
fn weights(request: &RewardDecompositionRequest, knobs: &RewardKnobs) -> Result<Vec<i128>, String> {
    let floor = i128::from(micros_to_q32(knobs.difficulty_floor_micros));
    let keep = ONE_MICROS - i128::from(knobs.cache_tier_discount_micros);
    let mut previous = 0i128;
    let mut out = Vec::with_capacity(request.steps.len());
    for signal in request.steps.iter() {
        let estimate = on_q32(&signal.value_estimate, "value_estimate")?;
        let weight = (estimate - previous).max(floor);
        previous = estimate;
        out.push(if signal.cache_hit {
            weight * keep / ONE_MICROS
        } else {
            weight
        });
    }
    Ok(out)
}

/// The normalised advantage `(reward - mean) / std` of every reward, on `Q32`.
/// A set whose rewards are all equal has no spread and every advantage is 0.
fn advantages(rewards: &[i128]) -> Vec<i128> {
    let count = rewards.len() as i128;
    let mean = rewards.iter().sum::<i128>().div_euclid(count);
    let variance = rewards
        .iter()
        .map(|r| (r - mean) * (r - mean))
        .sum::<i128>()
        / count;
    let std = isqrt(variance.unsigned_abs()) as i128;
    rewards
        .iter()
        .map(|r| {
            if std == 0 {
                0
            } else {
                ((r - mean) << 32) / std
            }
        })
        .collect()
}

/// Split the trajectory reward over its steps, or verify a proposed split.
pub fn decompose(
    request: &RewardDecompositionRequest,
    knobs: &RewardKnobs,
) -> Result<TrajectoryReward, String> {
    let total_reward = on_q32(&request.trajectory_reward, "trajectory_reward")?;
    let count = request.steps.len();
    if count == 0 || count > MAX_REWARD_STEPS {
        return Err(format!("steps must hold 1 to {MAX_REWARD_STEPS} entries"));
    }
    let weights = weights(request, knobs)?;
    let rewards: Vec<i128> = match &request.proposed_rewards {
        // A proposal is verified: it must sum to the trajectory reward within
        // the tolerance, and it is returned as proposed.
        Some(proposed) => {
            let proposed: Vec<i128> = proposed
                .iter()
                .map(|r| on_q32(r, "a proposed reward"))
                .collect::<Result<_, _>>()?;
            if proposed.len() != count {
                return Err("proposed_rewards must hold one reward per step".to_string());
            }
            let tolerance = i128::from(micros_to_q32(knobs.sum_tolerance_micros));
            if (proposed.iter().sum::<i128>() - total_reward).abs() > tolerance {
                return Err("the proposed rewards do not sum to the trajectory reward".to_string());
            }
            proposed
        }
        None => {
            let total_weight: i128 = weights.iter().sum();
            if total_weight <= 0 {
                return Err("the step weights vanish; lower reward.cache_tier_discount".to_string());
            }
            let mut rewards: Vec<i128> = weights[..count - 1]
                .iter()
                .map(|w| (total_reward * w).div_euclid(total_weight))
                .collect();
            let last = total_reward - rewards.iter().sum::<i128>();
            rewards.push(last);
            rewards
        }
    };
    let advantage = request.with_advantage.then(|| advantages(&rewards));
    let step_rewards = rewards
        .iter()
        .enumerate()
        .map(|(index, reward)| {
            Ok(StepReward {
                step: u32::try_from(index).map_err(|_| "too many steps".to_string())?,
                reward: q32(*reward)?,
                advantage: match &advantage {
                    Some(values) => Some(q32(values[index])?),
                    None => None,
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut prefix = vec![0i128];
    for reward in &rewards {
        prefix.push(prefix[prefix.len() - 1] + reward);
    }
    let mut node_rewards = Vec::with_capacity(request.hierarchy.len());
    for (index, node) in request.hierarchy.iter().enumerate() {
        let (first, last) = (node.first_step as usize, node.last_step as usize);
        if first > last || last >= count {
            return Err(format!(
                "hierarchy node {index} names steps outside the trace"
            ));
        }
        if node.parent.is_some_and(|parent| parent as usize >= index) {
            return Err(format!("hierarchy node {index} has no earlier parent"));
        }
        node_rewards.push(q32(prefix[last + 1] - prefix[first])?);
    }
    debug_assert!(node_rewards.len() <= MAX_HTN_NODES);
    let low = i128::from(micros_to_q32(knobs.saturation_low_micros));
    let high = i128::from(micros_to_q32(knobs.saturation_high_micros));
    Ok(TrajectoryReward {
        trajectory_id: request.trajectory_id.clone(),
        step_rewards: BoundedVec::new(step_rewards)?,
        node_rewards: BoundedVec::new(node_rewards)?,
        keep_for_training: low < total_reward && total_reward < high,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::reward::{HtnNode, StepSignal};

    const ONE: i64 = 1 << 32;

    fn q(value: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Q32,
            value,
        }
    }

    fn request(
        reward: i64,
        estimates: &[(i64, bool)],
        hierarchy: Vec<HtnNode>,
        with_advantage: bool,
    ) -> RewardDecompositionRequest {
        RewardDecompositionRequest::new(
            "traj-1".into(),
            q(reward),
            estimates
                .iter()
                .map(|(estimate, cache_hit)| StepSignal {
                    value_estimate: q(*estimate),
                    cache_hit: *cache_hit,
                })
                .collect(),
            hierarchy,
            with_advantage,
            None,
        )
        .unwrap()
    }

    fn rewards(answer: &TrajectoryReward) -> Vec<i64> {
        answer.step_rewards.iter().map(|r| r.reward.value).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R751.751
    #[test]
    fn rewards_sum_exactly_and_floor_applies() {
        let knobs = RewardKnobs::default();
        // Step 2 shows no progress: its weight is the 0.01 floor, not 0.
        let answer = decompose(
            &request(
                ONE,
                &[
                    (ONE / 4, false),
                    (ONE / 2, false),
                    (ONE / 2, false),
                    (ONE, false),
                ],
                Vec::new(),
                false,
            ),
            &knobs,
        )
        .unwrap();
        let got = rewards(&answer);
        assert_eq!(got.iter().sum::<i64>(), ONE, "the split is exact");
        assert!(got[2] > 0, "the floor keeps a stalled step positive");
        assert!(got[2] < got[0] / 10, "a stalled step earns far less");
        // An awkward total still sums exactly: the last step takes the rest.
        let odd = decompose(
            &request(
                ONE + 12_345,
                &[(1, false), (2, false), (7, false)],
                Vec::new(),
                false,
            ),
            &knobs,
        )
        .unwrap();
        assert_eq!(rewards(&odd).iter().sum::<i64>(), ONE + 12_345);
        // A negative reward splits exactly too.
        let negative = decompose(
            &request(
                -ONE,
                &[(ONE / 3, false), (ONE / 2, false)],
                Vec::new(),
                false,
            ),
            &knobs,
        )
        .unwrap();
        assert_eq!(rewards(&negative).iter().sum::<i64>(), -ONE);
        // Saturation: strictly inside (0, 1) keeps the trajectory for training.
        assert!(
            !answer.keep_for_training,
            "a reward of exactly 1 is saturated"
        );
        let inside = decompose(
            &request(ONE / 2, &[(ONE, false)], Vec::new(), false),
            &knobs,
        )
        .unwrap();
        assert!(inside.keep_for_training);
        // A node reward is the sum of its steps' rewards; a bad range is refused.
        let tree = decompose(
            &request(
                ONE,
                &[(ONE / 4, false), (ONE / 2, false), (ONE, false)],
                vec![
                    HtnNode {
                        parent: None,
                        first_step: 0,
                        last_step: 2,
                    },
                    HtnNode {
                        parent: Some(0),
                        first_step: 1,
                        last_step: 2,
                    },
                ],
                false,
            ),
            &knobs,
        )
        .unwrap();
        let steps = rewards(&tree);
        assert_eq!(tree.node_rewards.as_slice()[0].value, ONE);
        assert_eq!(tree.node_rewards.as_slice()[1].value, steps[1] + steps[2]);
        for bad in [
            HtnNode {
                parent: None,
                first_step: 2,
                last_step: 1,
            },
            HtnNode {
                parent: None,
                first_step: 0,
                last_step: 3,
            },
            HtnNode {
                parent: Some(0),
                first_step: 0,
                last_step: 1,
            },
        ] {
            assert!(decompose(
                &request(ONE, &[(1, false), (2, false), (3, false)], vec![bad], false),
                &knobs
            )
            .is_err());
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R751.751
    #[test]
    fn cache_hit_discounts_weight() {
        let knobs = RewardKnobs::default();
        let steps = |hit: bool| [(ONE / 2, false), (ONE, hit)];
        let cold =
            rewards(&decompose(&request(ONE, &steps(false), Vec::new(), false), &knobs).unwrap());
        let warm =
            rewards(&decompose(&request(ONE, &steps(true), Vec::new(), false), &knobs).unwrap());
        assert_eq!(cold[0], cold[1], "equal progress earns equal reward");
        assert!(
            warm[1] < warm[0],
            "a cache hit earns less than a fresh step"
        );
        // With the default 0.5 discount the hit step earns a third of the total.
        assert!((warm[1] - ONE / 3).abs() <= 1);
        assert_eq!(warm.iter().sum::<i64>(), ONE);
        // A full discount on every step leaves no weight to split.
        let full = RewardKnobs {
            cache_tier_discount_micros: 1_000_000,
            ..knobs
        };
        assert!(decompose(
            &request(ONE, &[(ONE / 2, true), (ONE, true)], Vec::new(), false),
            &full
        )
        .is_err());
        // A proposal is verified, not recomputed: it must sum to the total.
        let mut proposal = request(ONE, &steps(false), Vec::new(), false);
        proposal.proposed_rewards = Some(BoundedVec::new(vec![q(ONE / 4), q(ONE / 4)]).unwrap());
        assert!(decompose(&proposal, &knobs).is_err());
        proposal.proposed_rewards =
            Some(BoundedVec::new(vec![q(ONE / 4), q(ONE - ONE / 4)]).unwrap());
        assert_eq!(
            rewards(&decompose(&proposal, &knobs).unwrap()),
            vec![ONE / 4, ONE - ONE / 4]
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R751.751
    #[test]
    fn decomposition_is_idempotent() {
        let knobs = RewardKnobs::default();
        let req = request(
            ONE,
            &[
                (ONE / 4, false),
                (ONE / 2, true),
                (ONE / 2, false),
                (ONE, false),
            ],
            Vec::new(),
            true,
        );
        let first = decompose(&req, &knobs).unwrap();
        let again = decompose(&req, &knobs).unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&again).unwrap(),
            "repeated calls return identical bytes"
        );
        // Golden vector from an independent big-integer oracle of the row's
        // formula: weights 0.25, 0.125 (cache hit halves 0.25), 0.01, 0.5.
        assert_eq!(
            rewards(&first),
            vec![1213267597, 606633798, 48530703, 2426535198]
        );
        let advantage: Vec<i64> = first
            .step_rewards
            .iter()
            .map(|r| r.advantage.unwrap().value)
            .collect();
        assert_eq!(
            advantage,
            vec![678661107, -2272039375, -4986683820, 6580062088]
        );
        assert!(first.step_rewards.iter().all(|r| r.advantage.is_some()));
        // A constant split has no spread: every advantage is 0.
        let flat = decompose(
            &request(ONE, &[(ONE / 2, false), (ONE, false)], Vec::new(), true),
            &knobs,
        )
        .unwrap();
        assert!(flat.step_rewards.iter().all(|r| r.advantage == Some(q(0))));
    }

    #[test]
    fn isqrt_is_exact_on_squares_and_floors_between() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, 1 << 80, (1 << 80) + 1] {
            let root = isqrt(n);
            assert!(root * root <= n && (root + 1) * (root + 1) > n, "{n}");
        }
    }
}
