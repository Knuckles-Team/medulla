//! Branch outcome aggregation and breadth adaptation (MEDULLA-DECISION-ENGINE-R164.2).
//!
//! Pure functions over the types of
//! `medulla_decision_outcome_types::decision::branch_outcomes`. All arithmetic
//! is integer, so a replay is exact. Cost is O(observations).

use std::collections::BTreeMap;

use medulla_decision_outcome_types::decision::branch_outcomes::{
    BranchOutcome, BranchOutcomeObservation, BreadthAdaptation, BreadthPolicy, OutcomeEvidence,
};

/// One million: success rates are micro-units.
const RATE_ONE: u128 = 1_000_000;

/// Group `observations` by `branch_id`, ordered by `branch_id`. A group's
/// `average_cost` is the integer mean of its costs (rounded down).
pub fn retrieve_branch_outcomes(observations: &[BranchOutcomeObservation]) -> Vec<BranchOutcome> {
    let mut groups: BTreeMap<&str, (u64, u64, u128)> = BTreeMap::new();
    for observation in observations {
        let group = groups.entry(observation.branch_id.as_str()).or_default();
        group.0 += 1;
        group.1 += u64::from(observation.succeeded);
        group.2 += u128::from(observation.cost);
    }
    let mut by_id: BTreeMap<&str, &BranchOutcomeObservation> = BTreeMap::new();
    for observation in observations {
        by_id
            .entry(observation.branch_id.as_str())
            .or_insert(observation);
    }
    groups
        .into_iter()
        .map(|(id, (attempts, successes, cost))| BranchOutcome {
            branch_id: by_id[id].branch_id.clone(),
            attempt_count: attempts,
            success_count: successes,
            average_cost: u64::try_from(cost / u128::from(attempts)).unwrap_or(u64::MAX),
        })
        .collect()
}

/// The breadth to use next, and the adaptation that produced it when a branch
/// had enough attempts to decide on.
///
/// Branches below `policy.min_attempts` are ignored. The branch with the most
/// attempts decides, ties going to the lowest `branch_id`. A success rate at or
/// above `raise_at` adds one to `prior_breadth`; at or below `narrow_at`
/// subtracts one. The result is clamped to `[min, max]`. With no deciding
/// branch the breadth holds (still clamped) and no adaptation is returned.
pub fn adapt_breadth(
    outcomes: &[BranchOutcome],
    prior_breadth: u32,
    policy: &BreadthPolicy,
) -> (u32, Option<BreadthAdaptation>) {
    let clamp = |value: u32| value.clamp(policy.min, policy.max);
    let deciding = outcomes
        .iter()
        .filter(|o| o.attempt_count >= u64::from(policy.min_attempts) && o.attempt_count > 0)
        .min_by(|a, b| {
            b.attempt_count
                .cmp(&a.attempt_count)
                .then_with(|| a.branch_id.cmp(&b.branch_id))
        });
    let Some(outcome) = deciding else {
        return (clamp(prior_breadth), None);
    };
    let rate = u128::from(outcome.success_count) * RATE_ONE / u128::from(outcome.attempt_count);
    let proposed = if rate >= u128::from(policy.raise_at) {
        prior_breadth.saturating_add(1)
    } else if rate <= u128::from(policy.narrow_at) {
        prior_breadth.saturating_sub(1)
    } else {
        prior_breadth
    };
    let new_breadth = clamp(proposed);
    let adaptation = BreadthAdaptation {
        branch_id: outcome.branch_id.clone(),
        prior_breadth,
        new_breadth,
        outcome_triggering: OutcomeEvidence {
            prior_branch_id: outcome.branch_id.clone(),
            outcome: outcome.clone(),
            adaptation_applied: new_breadth != prior_breadth,
        },
    };
    (new_breadth, Some(adaptation))
}

#[cfg(test)]
mod tests {
    use medulla_types::contract::OpaqueId;

    use super::*;

    fn observation(branch: &str, succeeded: bool, cost: u64) -> BranchOutcomeObservation {
        BranchOutcomeObservation {
            branch_id: OpaqueId::new(branch).unwrap(),
            succeeded,
            cost,
        }
    }

    fn attempts(
        branch: &str,
        successes: u64,
        failures: u64,
        cost: u64,
    ) -> Vec<BranchOutcomeObservation> {
        let mut out = Vec::new();
        out.extend((0..successes).map(|_| observation(branch, true, cost)));
        out.extend((0..failures).map(|_| observation(branch, false, cost)));
        out
    }

    fn policy() -> BreadthPolicy {
        BreadthPolicy {
            min: 2,
            max: 6,
            raise_at: 700_000,
            narrow_at: 300_000,
            min_attempts: 5,
        }
    }

    fn breadth(observations: &[BranchOutcomeObservation], prior: u32) -> u32 {
        adapt_breadth(&retrieve_branch_outcomes(observations), prior, &policy()).0
    }

    // spec: MEDULLA-DECISION-ENGINE-R164.2
    #[test]
    fn breadth_adapts_from_outcomes() {
        // Grouping: counts and the integer mean cost, ordered by branch id.
        let mut mixed = vec![
            observation("b", true, 10),
            observation("a", false, 7),
            observation("b", false, 15),
            observation("a", true, 8),
            observation("a", true, 8),
        ];
        let grouped = retrieve_branch_outcomes(&mixed);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].branch_id.as_str(), "a");
        assert_eq!(
            (
                grouped[0].attempt_count,
                grouped[0].success_count,
                grouped[0].average_cost
            ),
            (3, 2, 7)
        );
        assert_eq!(
            (
                grouped[1].attempt_count,
                grouped[1].success_count,
                grouped[1].average_cost
            ),
            (2, 1, 12)
        );
        mixed.clear();
        assert!(retrieve_branch_outcomes(&mixed).is_empty());

        // Raise at the threshold (7 of 10), narrow at it (3 of 10), hold between.
        assert_eq!(breadth(&attempts("a", 7, 3, 1), 4), 5);
        assert_eq!(breadth(&attempts("a", 3, 7, 1), 4), 3);
        assert_eq!(breadth(&attempts("a", 5, 5, 1), 4), 4);
        // Clamp at both bounds.
        assert_eq!(breadth(&attempts("a", 10, 0, 1), 6), 6);
        assert_eq!(breadth(&attempts("a", 0, 10, 1), 2), 2);
        assert_eq!(
            breadth(&attempts("a", 5, 5, 1), 9),
            6,
            "a hold is clamped too"
        );
        // Below min_attempts the branch is ignored and nothing adapts.
        let few = attempts("a", 4, 0, 1);
        let (held, adaptation) = adapt_breadth(&retrieve_branch_outcomes(&few), 4, &policy());
        assert_eq!((held, adaptation), (4, None));
        // The adaptation names the deciding branch and whether it applied.
        let raised = attempts("a", 8, 2, 1);
        let (_, adaptation) = adapt_breadth(&retrieve_branch_outcomes(&raised), 4, &policy());
        let adaptation = adaptation.unwrap();
        assert_eq!(adaptation.branch_id.as_str(), "a");
        assert_eq!((adaptation.prior_breadth, adaptation.new_breadth), (4, 5));
        assert!(adaptation.outcome_triggering.adaptation_applied);
        let (_, at_max) = adapt_breadth(
            &retrieve_branch_outcomes(&attempts("a", 10, 0, 1)),
            6,
            &policy(),
        );
        assert!(!at_max.unwrap().outcome_triggering.adaptation_applied);
    }

    // spec: MEDULLA-DECISION-ENGINE-R164.2
    #[test]
    fn ties_resolve_to_the_lowest_branch_id() {
        // Two branches with the same attempt count disagree: the lowest id decides.
        let mut high_wins = attempts("b", 10, 0, 1);
        high_wins.extend(attempts("a", 0, 10, 1));
        let mut reversed = attempts("a", 0, 10, 1);
        reversed.extend(attempts("b", 10, 0, 1));
        for observations in [&high_wins, &reversed] {
            let (new, adaptation) =
                adapt_breadth(&retrieve_branch_outcomes(observations), 4, &policy());
            assert_eq!(
                new, 3,
                "branch a (all failures) decides, so breadth narrows"
            );
            assert_eq!(adaptation.unwrap().branch_id.as_str(), "a");
        }
        // More attempts beat a lower id.
        let mut busier = attempts("a", 0, 5, 1);
        busier.extend(attempts("z", 12, 0, 1));
        let (new, adaptation) = adapt_breadth(&retrieve_branch_outcomes(&busier), 4, &policy());
        assert_eq!(new, 5);
        assert_eq!(adaptation.unwrap().branch_id.as_str(), "z");
        // Input order never changes the result.
        let mut shuffled = busier.clone();
        shuffled.reverse();
        assert_eq!(
            retrieve_branch_outcomes(&busier),
            retrieve_branch_outcomes(&shuffled)
        );
    }
}
