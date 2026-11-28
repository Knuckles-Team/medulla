//! Population drift and fan-out width kernels (MEDULLA-DECISION-ENGINE-R752.751).
//!
//! [`wasserstein1`] is the first Wasserstein distance between two score
//! windows, read on a common quantile grid; [`drift_reading`] alerts when it
//! exceeds `knob drift.alert`. [`fanout_width`] picks how many replicas to fan
//! out from their mean diversity: up one at `knob fanout.diversity_high`, down
//! one (never below 1) at `knob fanout.diversity_low`, and never above the cap
//! the budget and `knob fanout.replica_ceiling` allow. All arithmetic is
//! fixed-point integer on `Q32`; no float, no clock, no store.

use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use {
    medulla_decision_policy_types::decision::population::DriftReading,
    medulla_decision_policy_types::decision::population::PopulationKnobs,
    medulla_decision_policy_types::decision::population::PopulationSpread,
    medulla_decision_policy_types::decision::population::Wasserstein1,
};
use {
    medulla_decision_statistical_types::decision::numeric::micros_to_q32,
    medulla_decision_statistical_types::decision::numeric::QuantScaleTag,
    medulla_decision_statistical_types::decision::numeric::QuantisedValue,
};

fn sorted_q32(window: &[QuantisedValue], what: &str) -> Result<Vec<i128>, String> {
    if window.is_empty() {
        return Err(format!("the {what} window is empty"));
    }
    let mut out = Vec::with_capacity(window.len());
    for sample in window {
        if sample.scale != QuantScaleTag::Q32 {
            return Err(format!("the {what} window holds a sample off scale Q32"));
        }
        out.push(i128::from(sample.value));
    }
    out.sort_unstable();
    Ok(out)
}

/// The first Wasserstein distance between two windows, on `Q32`.
///
/// Both windows sort ascending. With `m` the larger length, quantile `k` of a
/// window of length `n` is element `floor((2k + 1) * n / (2m))`. The distance
/// is the mean absolute quantile difference over `k` in `0..m`, rounded half up.
pub fn wasserstein1(
    reference: &[QuantisedValue],
    current: &[QuantisedValue],
) -> Result<Wasserstein1, String> {
    let (a, b) = (
        sorted_q32(reference, "reference")?,
        sorted_q32(current, "current")?,
    );
    let m = a.len().max(b.len());
    let mut sum: i128 = 0;
    for k in 0..m {
        let at = |window: &[i128]| window[(2 * k + 1) * window.len() / (2 * m)];
        sum += (at(&a) - at(&b)).abs();
    }
    let m = m as i128;
    let rounded = (2 * sum + m) / (2 * m);
    Ok(Wasserstein1 {
        value: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: i64::try_from(rounded).map_err(|_| "the distance overflows Q32".to_string())?,
        },
    })
}

/// One drift reading: the distance, both window sizes and whether it alerts.
pub fn drift_reading(
    reference: &[QuantisedValue],
    current: &[QuantisedValue],
    knobs: &PopulationKnobs,
) -> Result<DriftReading, String> {
    let distance = wasserstein1(reference, current)?;
    let alert = distance.value.value > micros_to_q32(knobs.drift_alert_micros);
    Ok(DriftReading {
        distance,
        reference_n: u32::try_from(reference.len()).map_err(|_| "window too long".to_string())?,
        current_n: u32::try_from(current.len()).map_err(|_| "window too long".to_string())?,
        alert,
    })
}

/// The mean of the pairwise diversity entries, on `Q32`, rounded down.
pub fn mean_diversity(spread: &PopulationSpread) -> i64 {
    let entries = spread.pairwise.as_slice();
    let sum: i128 = entries.iter().map(|d| i128::from(d.value)).sum();
    i64::try_from(sum / entries.len().max(1) as i128).unwrap_or(i64::MAX)
}

/// The most replicas the budget and `knob fanout.replica_ceiling` allow; 0
/// when the budget does not cover one replica or a replica costs nothing.
pub fn fanout_cap(budget: u64, cost_per_replica: u64, knobs: &PopulationKnobs) -> u32 {
    let affordable = budget
        .checked_div(cost_per_replica)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    knobs.replica_ceiling.min(affordable)
}

/// The replica width to fan out: one more at high diversity, one fewer (to a
/// minimum of 1) at low diversity, otherwise the current width, and never above
/// `min(knob fanout.replica_ceiling, floor(budget / cost_per_replica))`.
pub fn fanout_width(
    spread: &PopulationSpread,
    current_width: u32,
    budget: u64,
    cost_per_replica: u64,
    knobs: &PopulationKnobs,
) -> Result<u32, String> {
    spread.validate()?;
    let invalid = |detail: &str| StatisticalErrorCode::ParameterInvalid.refusal(detail);
    if current_width == 0 {
        return Err(invalid("current_width must be at least 1"));
    }
    if cost_per_replica == 0 {
        return Err(invalid("cost_per_replica must be at least 1"));
    }
    let cap = fanout_cap(budget, cost_per_replica, knobs);
    if cap == 0 {
        return Err(invalid("the budget cannot fund one replica"));
    }
    let diversity = mean_diversity(spread);
    let wanted = if diversity >= micros_to_q32(knobs.diversity_high_micros) {
        current_width.saturating_add(1)
    } else if diversity <= micros_to_q32(knobs.diversity_low_micros) {
        current_width.saturating_sub(1).max(1)
    } else {
        current_width
    };
    Ok(wanted.min(cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i64 = 1 << 32;

    fn q(value: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Q32,
            value,
        }
    }

    fn window(values: &[i64]) -> Vec<QuantisedValue> {
        values.iter().map(|v| q(*v)).collect()
    }

    fn spread(diversity: i64) -> PopulationSpread {
        PopulationSpread::new(4, vec![q(diversity); 6]).unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R752.751
    #[test]
    fn wasserstein_matches_hand_computed_cases() {
        let distance =
            |a: &[i64], b: &[i64]| wasserstein1(&window(a), &window(b)).unwrap().value.value;
        // Identical windows are at distance 0, whatever the order.
        assert_eq!(distance(&[5, 1, 3], &[3, 5, 1]), 0);
        // One sample against a spread: |0 - 1/2| and |1 - 1/2| average to 1/2.
        assert_eq!(distance(&[0, ONE], &[ONE / 2]), ONE / 2);
        // A shift of the whole window by 1/4 is a distance of exactly 1/4.
        assert_eq!(
            distance(&[0, ONE / 4, ONE / 2], &[ONE / 4, ONE / 2, 3 * ONE / 4]),
            ONE / 4
        );
        // Unequal lengths: the quantile grid has 6 points, the reference
        // reads elements 0,1,1,2,3,3 -> .1 .2 .2 .3 .4 .4 against .3 .4 .5 .6
        // .7 .8, a mean difference of 1.7/6; the golden is the integer oracle.
        let reference = [429496729, 858993459, 1288490188, 1717986918];
        let current = [
            1288490188, 1717986918, 2147483648, 2576980377, 3006477107, 3435973836,
        ];
        assert_eq!(distance(&reference, &current), 1216907401);
        // The distance is symmetric.
        assert_eq!(distance(&current, &reference), 1216907401);
        // An empty window and a sample off scale Q32 are refused.
        assert!(wasserstein1(&[], &window(&[1])).is_err());
        assert!(wasserstein1(
            &window(&[1]),
            &[QuantisedValue {
                scale: QuantScaleTag::Pico,
                value: 1
            }]
        )
        .is_err());
        // The alert fires strictly above the 0.10 knob.
        let knobs = PopulationKnobs::from_policy(
            &medulla_decision_statistical_types::decision::DecisionPolicy::engine_default(),
        );
        let near =
            drift_reading(&window(&[0]), &window(&[micros_to_q32(100_000)]), &knobs).unwrap();
        assert!(
            !near.alert,
            "a distance equal to the threshold does not alert"
        );
        let far = drift_reading(
            &window(&[0]),
            &window(&[micros_to_q32(100_000) + 1]),
            &knobs,
        )
        .unwrap();
        assert!(far.alert);
        assert_eq!((far.reference_n, far.current_n), (1, 1));
    }

    // spec: MEDULLA-DECISION-ENGINE-R752.751
    #[test]
    fn width_never_exceeds_the_cap() {
        let knobs = PopulationKnobs::from_policy(
            &medulla_decision_statistical_types::decision::DecisionPolicy::engine_default(),
        );
        assert_eq!(knobs.replica_ceiling, 8);
        let width = |diversity, current, budget| {
            fanout_width(&spread(diversity), current, budget, 10, &knobs)
        };
        // High diversity grows by one, low shrinks by one (floor 1), the
        // middle holds.
        assert_eq!(width(ONE / 2, 3, 100), Ok(4));
        assert_eq!(width(ONE / 10, 3, 100), Ok(2));
        assert_eq!(width(ONE / 10, 1, 100), Ok(1));
        assert_eq!(width(ONE / 4, 3, 100), Ok(3));
        // The thresholds themselves count: >= high grows, <= low shrinks.
        assert_eq!(width(micros_to_q32(400_000), 3, 100), Ok(4));
        assert_eq!(width(micros_to_q32(150_000), 3, 100), Ok(2));
        // The ceiling caps the growth, and so does the budget.
        assert_eq!(width(ONE / 2, 8, 1000), Ok(8));
        assert_eq!(width(ONE / 2, 5, 40), Ok(4));
        assert_eq!(
            width(ONE / 2, 20, 1000),
            Ok(8),
            "a width above the cap is cut to it"
        );
        for current in 1..=12 {
            for budget in [10, 25, 80, 1000] {
                for diversity in [0, ONE / 4, ONE] {
                    let got = width(diversity, current, budget).unwrap();
                    assert!(got <= 8 && u64::from(got) <= budget / 10 && got >= 1);
                }
            }
        }
        // A budget that cannot fund one replica, and a zero cost, are refused.
        assert!(width(ONE / 2, 3, 9).is_err());
        assert!(fanout_width(&spread(ONE / 2), 3, 100, 0, &knobs).is_err());
        assert!(fanout_width(&spread(ONE / 2), 0, 100, 10, &knobs).is_err());
        // The mean diversity is the mean of the pairwise entries.
        let mixed = PopulationSpread::new(2, vec![q(ONE / 2)]).unwrap();
        assert_eq!(mean_diversity(&mixed), ONE / 2);
    }

    // spec: MEDULLA-DECISION-ENGINE-R752.751
    #[test]
    fn drift_and_width_replay_golden_vectors() {
        let knobs = PopulationKnobs::from_policy(
            &medulla_decision_statistical_types::decision::DecisionPolicy::engine_default(),
        );
        let reference = window(&[429496729, 858993459, 1288490188, 1717986918]);
        let current = window(&[
            1288490188, 1717986918, 2147483648, 2576980377, 3006477107, 3435973836,
        ]);
        let first = drift_reading(&reference, &current, &knobs).unwrap();
        let second = drift_reading(&reference, &current, &knobs).unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap(),
            "a replay returns identical bytes"
        );
        assert_eq!(first.distance.value.value, 1216907401);
        assert!(first.alert);
        assert_eq!((first.reference_n, first.current_n), (4, 6));
        // The width replays the same way: a mixed spread whose mean is 0.45.
        let pairwise = window(&[ONE / 4, ONE / 2, 3 * ONE / 4, ONE / 4, ONE / 2, 3 * ONE / 4]);
        let spread = PopulationSpread::new(4, pairwise).unwrap();
        assert_eq!(mean_diversity(&spread), ONE / 2);
        let widths: Vec<u32> = (0..3)
            .map(|_| fanout_width(&spread, 5, 80, 10, &knobs).unwrap())
            .collect();
        assert_eq!(widths, vec![6, 6, 6]);
    }
}
