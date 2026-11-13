//! Ranking routing paths by their recent success rate (MEDULLA-DECISION-ENGINE-R159.2).
//!
//! The rate is successes over total attempts inside the window. A path is
//! eligible when its attempts reach `path.min_samples`, preferred when its rate
//! reaches `path.prefer_above_rate`, and skipped when its latest
//! `path.skip_after_failures` evaluations all failed. The order is rate
//! descending, then total attempts descending, then `path_id` ascending; the
//! ranked list is also the fallback order. The function is pure.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use medulla_decision_types::decision::path_success::{PathPreferenceRule, SuccessRateEvidence};

/// One ratio is `1_000_000` millionths.
const MICROS: u64 = 1_000_000;

/// One path in the ranked, fallback-ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedPath {
    /// The path identity.
    pub path_id: String,
    /// Successful attempts inside the window.
    pub successful_attempts: u32,
    /// All attempts inside the window.
    pub total_attempts: u32,
    /// The success rate in millionths, rounded down.
    pub rate_micros: u32,
    /// Whether the rate reaches the preference threshold.
    pub preferred: bool,
}

/// Rank the eligible, unskipped paths. `recent` holds each path's latest
/// evaluations, newest first (`true` is a success). Evidence over a longer
/// window than the rule's, below the minimum attempts, or whose latest
/// evaluations all failed is left out.
pub fn rank_paths(
    evidence: &[SuccessRateEvidence],
    recent: &BTreeMap<String, Vec<bool>>,
    rule: &PathPreferenceRule,
) -> Vec<RankedPath> {
    let mut ranked: Vec<RankedPath> = evidence
        .iter()
        .filter(|e| e.window_ms <= rule.window_ms)
        .filter(|e| e.total_attempts > 0 && e.total_attempts >= rule.min_samples)
        .filter(|e| !skipped(recent.get(&e.path_id), rule.skip_after_failures))
        .map(|e| RankedPath {
            path_id: e.path_id.clone(),
            successful_attempts: e.successful_attempts,
            total_attempts: e.total_attempts,
            rate_micros: ((u64::from(e.successful_attempts) * MICROS) / u64::from(e.total_attempts))
                as u32,
            preferred: u64::from(e.successful_attempts) * MICROS
                >= u64::from(rule.prefer_above_rate) * u64::from(e.total_attempts),
        })
        .collect();
    ranked.sort_by(compare);
    ranked
}

/// The latest `after` evaluations all failed.
fn skipped(recent: Option<&Vec<bool>>, after: u32) -> bool {
    let Some(recent) = recent else { return false };
    let after = after as usize;
    after > 0 && recent.len() >= after && recent[..after].iter().all(|ok| !ok)
}

/// Rate descending (exact, by cross-multiplication), then total attempts
/// descending, then `path_id` ascending.
fn compare(a: &RankedPath, b: &RankedPath) -> Ordering {
    let left = u64::from(a.successful_attempts) * u64::from(b.total_attempts);
    let right = u64::from(b.successful_attempts) * u64::from(a.total_attempts);
    right
        .cmp(&left)
        .then_with(|| b.total_attempts.cmp(&a.total_attempts))
        .then_with(|| a.path_id.cmp(&b.path_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PathPreferenceRule {
        PathPreferenceRule {
            window_ms: 86_400_000,
            min_samples: 10,
            prefer_above_rate: 500_000,
            skip_after_failures: 3,
        }
    }

    fn evidence(id: &str, ok: u32, total: u32) -> SuccessRateEvidence {
        SuccessRateEvidence {
            path_id: id.into(),
            successful_attempts: ok,
            total_attempts: total,
            window_ms: 86_400_000,
        }
    }

    fn ids(ranked: &[RankedPath]) -> Vec<&str> {
        ranked.iter().map(|p| p.path_id.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R159.2
    #[test]
    fn success_preference_gates() {
        let none = BTreeMap::new();
        // Eligibility: fewer attempts than min_samples is left out.
        let ranked = rank_paths(
            &[evidence("a", 9, 9), evidence("b", 10, 10)],
            &none,
            &rule(),
        );
        assert_eq!(ids(&ranked), vec!["b"]);
        // Preference: the threshold is inclusive.
        let ranked = rank_paths(
            &[
                evidence("half", 5, 10),
                evidence("under", 4, 10),
                evidence("full", 10, 10),
            ],
            &none,
            &rule(),
        );
        assert_eq!(ids(&ranked), vec!["full", "half", "under"]);
        assert_eq!(
            ranked.iter().map(|p| p.preferred).collect::<Vec<_>>(),
            vec![true, true, false]
        );
        assert_eq!(ranked[1].rate_micros, 500_000);
        // Window: evidence over a longer window than the rule's is not used.
        let mut old = evidence("old", 10, 10);
        old.window_ms = 86_400_001;
        assert!(rank_paths(&[old], &none, &rule()).is_empty());
        // Skip: the latest three evaluations all failed.
        let recent = BTreeMap::from([
            ("a".to_string(), vec![false, false, false, true]),
            ("b".to_string(), vec![false, false, true]),
            ("c".to_string(), vec![false, false]),
        ]);
        let ranked = rank_paths(
            &[
                evidence("a", 10, 10),
                evidence("b", 9, 10),
                evidence("c", 8, 10),
            ],
            &recent,
            &rule(),
        );
        assert_eq!(
            ids(&ranked),
            vec!["b", "c"],
            "only a has three failures in a row"
        );
        // Tie rules: rate, then more attempts, then path id.
        let ranked = rank_paths(
            &[
                evidence("z", 10, 20),
                evidence("b", 5, 10),
                evidence("a", 5, 10),
                evidence("hi", 15, 20),
            ],
            &none,
            &rule(),
        );
        assert_eq!(ids(&ranked), vec!["hi", "z", "a", "b"]);
        // No evidence, no ranking.
        assert!(rank_paths(&[], &none, &rule()).is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R159.2
    #[test]
    fn ranking_is_stable_under_input_order() {
        let none = BTreeMap::new();
        let items: Vec<SuccessRateEvidence> = (0..12u32)
            .map(|i| evidence(&format!("p{i:02}"), 10 + (i % 4), 20 + (i % 3)))
            .collect();
        let expected = rank_paths(&items, &none, &rule());
        // Deterministic permutations: rotations and a reversal.
        for shift in 0..items.len() {
            let mut rotated = items.clone();
            rotated.rotate_left(shift);
            assert_eq!(rank_paths(&rotated, &none, &rule()), expected);
        }
        let mut reversed = items.clone();
        reversed.reverse();
        assert_eq!(rank_paths(&reversed, &none, &rule()), expected);
        // Equal rate and attempts fall back to path id, whatever the order.
        let tied = [
            evidence("m", 5, 10),
            evidence("k", 5, 10),
            evidence("x", 5, 10),
        ];
        let mut shuffled = tied.to_vec();
        shuffled.swap(0, 2);
        assert_eq!(
            ids(&rank_paths(&shuffled, &none, &rule())),
            vec!["k", "m", "x"]
        );
    }
}
