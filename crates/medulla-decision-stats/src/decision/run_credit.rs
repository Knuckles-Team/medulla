//! The run credit kernel (MEDULLA-DECISION-ENGINE-R100.102.2): pure functions
//! over already-visibility-filtered retrieval outcomes.
//!
//! Cited rows are positives. Returned rows the answer does not cite are hard
//! negatives, computed by [`super::retrieval::hard_negatives`] over each
//! referenced decision's stored outcome. Each step's advantage is its reward
//! minus its capability's baseline, read before that step's update; the
//! baseline then moves toward the reward by `w_q16 / 65536` of the gap,
//! rounded half to even. All arithmetic is integer, so a replay is exact.
//! Cost is O(s + r) in steps and rows.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::statistical::retrieval::RetrievalOutcome;
use medulla_decision_types::decision::statistical::run_credit::StepRecord;
use medulla_types::contract::BoundedVec;

use super::retrieval::hard_negatives;

/// One capability's stored baseline: its value in Q16 and its revision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Baseline {
    pub value_q16: i32,
    pub revision: u32,
}

/// The credit one step earned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepAdvantage {
    pub step_idx: u32,
    pub advantage_q16: i32,
    /// The revision of the baseline the advantage was measured against.
    pub baseline_revision: u32,
}

/// One new baseline revision a step produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineUpdate {
    pub capability_iri: String,
    pub baseline: Baseline,
}

/// What the kernel computed for one run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunCredit {
    /// Cited rows that a referenced decision returned, sorted.
    pub positives: Vec<String>,
    /// Returned, uncited rows that outranked a cited one, sorted and distinct.
    pub negatives: Vec<String>,
    pub advantages: Vec<StepAdvantage>,
    /// One entry per step, in step order, each at the revision it creates.
    pub updates: Vec<BaselineUpdate>,
}

/// `numerator / 65536` rounded half to even.
fn div_q16_half_even(numerator: i64) -> i64 {
    let quotient = numerator.div_euclid(65_536);
    let remainder = numerator.rem_euclid(65_536);
    match remainder.cmp(&32_768) {
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => quotient + (quotient & 1),
        std::cmp::Ordering::Less => quotient,
    }
}

/// Credit one run.
///
/// `outcomes` are the stored retrieval outcomes of the referenced decisions,
/// `cited` the rows the answer cited, `baselines` the stored baselines by
/// capability (absent = value 0, revision 0) and `w_q16` the baseline weight
/// (`knob run_credit.baseline_weight`) as an integer over 65536. A run that
/// did not succeed credits no rows: its citations are not evidence of
/// relevance; its steps are still credited.
pub fn credit_steps(
    outcomes: &[&RetrievalOutcome],
    cited: &[String],
    success: bool,
    steps: &[StepRecord],
    baselines: &BTreeMap<String, Baseline>,
    w_q16: i64,
) -> RunCredit {
    let mut credit = RunCredit::default();
    if success {
        let mut positives = BTreeSet::new();
        let mut negatives = BTreeSet::new();
        for outcome in outcomes {
            let returned: BTreeSet<&str> = outcome
                .returned
                .iter()
                .map(|unit| unit.evidence_id.as_str())
                .collect();
            let used: Vec<String> = cited
                .iter()
                .filter(|id| returned.contains(id.as_str()))
                .cloned()
                .collect();
            positives.extend(used.iter().cloned());
            let mut view = (*outcome).clone();
            view.cited = BoundedVec::new(used).unwrap_or_default();
            negatives.extend(
                hard_negatives(&view)
                    .into_iter()
                    .map(|negative| negative.evidence_id),
            );
        }
        credit.positives = positives.into_iter().collect();
        credit.negatives = negatives.into_iter().collect();
    }
    let mut working = baselines.clone();
    for step in steps {
        let before = working
            .get(&step.capability_iri)
            .copied()
            .unwrap_or_default();
        let gap = i64::from(step.reward_q16) - i64::from(before.value_q16);
        let moved = i64::from(before.value_q16) + div_q16_half_even(w_q16 * gap);
        let after = Baseline {
            value_q16: i32::try_from(moved).unwrap_or(if moved < 0 { i32::MIN } else { i32::MAX }),
            revision: before.revision + 1,
        };
        credit.advantages.push(StepAdvantage {
            step_idx: step.step_idx,
            advantage_q16: i32::try_from(gap).unwrap_or(i32::MAX),
            baseline_revision: before.revision,
        });
        credit.updates.push(BaselineUpdate {
            capability_iri: step.capability_iri.clone(),
            baseline: after,
        });
        working.insert(step.capability_iri.clone(), after);
    }
    credit
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::retrieval::ReturnedEvidence;

    fn outcome(returned: &[&str]) -> RetrievalOutcome {
        RetrievalOutcome {
            record_id: "d1".to_string(),
            returned: BoundedVec::new(
                returned
                    .iter()
                    .map(|id| ReturnedEvidence {
                        evidence_id: id.to_string(),
                        content_class: None,
                    })
                    .collect(),
            )
            .unwrap(),
            cited: BoundedVec::default(),
            query: None,
            path: None,
        }
    }

    fn step(idx: u32, capability: &str, reward: i32) -> StepRecord {
        StepRecord {
            step_idx: idx,
            agent_iri: "urn:agent:a".to_string(),
            capability_iri: capability.to_string(),
            reward_q16: reward,
        }
    }

    #[test]
    fn half_even_rounding_is_exact() {
        assert_eq!(div_q16_half_even(32_768), 0);
        assert_eq!(div_q16_half_even(98_304), 2);
        assert_eq!(div_q16_half_even(-32_768), 0);
        assert_eq!(div_q16_half_even(-98_304), -2);
        assert_eq!(div_q16_half_even(32_769), 1);
        assert_eq!(div_q16_half_even(32_767), 0);
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.102.2
    #[test]
    fn baseline_moves_by_the_weighted_difference() {
        let first = outcome(&["a", "b", "c", "d"]);
        let second = outcome(&["b", "z"]);
        let cited = vec!["c".to_string()];
        let steps = vec![
            step(0, "urn:cap:x", 65_536),
            step(1, "urn:cap:x", 0),
            step(2, "urn:cap:y", -65_536),
        ];
        let stored = BTreeMap::from([(
            "urn:cap:y".to_string(),
            Baseline {
                value_q16: 32_768,
                revision: 4,
            },
        )]);
        let credit = credit_steps(&[&first, &second], &cited, true, &steps, &stored, 6_554);
        assert_eq!(credit.positives, ["c"]);
        // a, b outranked the cited c in the first decision; the second returned
        // no cited row, so it adds none.
        assert_eq!(credit.negatives, ["a", "b"]);
        // x: baseline 0 -> 0 + round(6554 * 65536 / 65536) = 6554.
        // second x step: gap = 0 - 6554, move = round(6554 * -6554 / 65536) = -655.
        // y: gap = -65536 - 32768 = -98304; move = round(6554 * -98304 / 65536) = -9831.
        let advantages: Vec<(u32, i32, u32)> = credit
            .advantages
            .iter()
            .map(|a| (a.step_idx, a.advantage_q16, a.baseline_revision))
            .collect();
        assert_eq!(
            advantages,
            [(0, 65_536, 0), (1, -6_554, 1), (2, -98_304, 4)]
        );
        let moved: Vec<(&str, i32, u32)> = credit
            .updates
            .iter()
            .map(|u| {
                (
                    u.capability_iri.as_str(),
                    u.baseline.value_q16,
                    u.baseline.revision,
                )
            })
            .collect();
        assert_eq!(
            moved,
            [
                ("urn:cap:x", 6_554, 1),
                ("urn:cap:x", 5_899, 2),
                ("urn:cap:y", 22_937, 5),
            ]
        );
        // Replay is bit for bit.
        assert_eq!(
            credit,
            credit_steps(&[&first, &second], &cited, true, &steps, &stored, 6_554)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.102.2
    #[test]
    fn a_failed_run_credits_steps_but_no_rows() {
        let first = outcome(&["a", "c"]);
        let credit = credit_steps(
            &[&first],
            &["c".to_string()],
            false,
            &[step(0, "urn:cap:x", 100)],
            &BTreeMap::new(),
            6_554,
        );
        assert!(credit.positives.is_empty() && credit.negatives.is_empty());
        assert_eq!(credit.advantages.len(), 1);
    }
}
