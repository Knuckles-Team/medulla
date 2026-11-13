//! Ingest shortfall allocation (MEDULLA-DECISION-ENGINE-R826).
//!
//! Given the measured throughput window, the declared goals (cost and the
//! papers each is expected to complete) and the remaining budget, the family
//! funds goals to maximise completed papers within the budget: cheapest cost
//! per paper first, ties broken by ascending goal id, a goal that no longer
//! fits skipped. A purge candidate is eligible only when it is terminal,
//! unleased and non-authoritative, keeps its source and decision receipts, is
//! not the last source copy and carries no active work (a `Fixed` predicate with
//! no knob). Advisory; `Decide` writes nothing.

use serde::{Deserialize, Serialize};

use medulla_types::ingest_throughput::ThroughputWindow;

/// JSON text parameter: the [`ThroughputWindow`] being repaired.
pub const PARAM_WINDOW: &str = "window";
/// JSON text parameter: the declared goals, an array of [`Goal`].
pub const PARAM_GOALS: &str = "goals";
/// JSON text parameter: the purge candidates, an array of [`PurgeCandidate`].
pub const PARAM_PURGE_CANDIDATES: &str = "purge_candidates";
/// Integer parameter: the budget that remains.
pub const PARAM_BUDGET: &str = "budget";
/// The one option an ingest shortfall question declares.
pub const ALLOCATION_OPTION: &str = "allocation";
/// Answer prefix of a funded goal.
pub const GOAL_PREFIX: &str = "goal:";
/// Answer prefix of an eligible purge candidate.
pub const PURGE_PREFIX: &str = "purge:";

/// One declared goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub goal_id: String,
    pub cost: u64,
    pub expected_papers: u64,
}

/// One item that could be purged to free room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeCandidate {
    pub item_id: String,
    pub terminal: bool,
    pub leased: bool,
    pub authoritative: bool,
    pub source_receipt: bool,
    pub decision_receipt: bool,
    pub last_source_copy: bool,
    pub active_work: bool,
}

/// The purge predicate: every condition must hold.
pub fn purge_eligible(candidate: &PurgeCandidate) -> bool {
    candidate.terminal
        && !candidate.leased
        && !candidate.authoritative
        && candidate.source_receipt
        && candidate.decision_receipt
        && !candidate.last_source_copy
        && !candidate.active_work
}

/// The goals funded within `budget`, in funding order.
pub fn allocate(goals: &[Goal], budget: u64) -> Vec<&Goal> {
    let mut order: Vec<&Goal> = goals.iter().filter(|g| g.expected_papers > 0).collect();
    // cost_a / papers_a < cost_b / papers_b, compared without division.
    order.sort_by(|a, b| {
        (u128::from(a.cost) * u128::from(b.expected_papers))
            .cmp(&(u128::from(b.cost) * u128::from(a.expected_papers)))
            .then_with(|| a.goal_id.cmp(&b.goal_id))
    });
    let mut left = budget;
    let mut funded = Vec::new();
    for goal in order {
        if goal.cost <= left {
            left -= goal.cost;
            funded.push(goal);
        }
    }
    funded
}

/// The eligible purge candidates, by ascending item id.
pub fn purges(candidates: &[PurgeCandidate]) -> Vec<&PurgeCandidate> {
    let mut eligible: Vec<&PurgeCandidate> =
        candidates.iter().filter(|c| purge_eligible(c)).collect();
    eligible.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    eligible
}

/// The fact the window contributes to the record.
pub fn window_fact(window: &ThroughputWindow) -> String {
    format!(
        "shortfall:{}:{}",
        window.shortfall,
        serde_json::to_string(&window.state).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: &str, cost: u64, papers: u64) -> Goal {
        Goal {
            goal_id: id.into(),
            cost,
            expected_papers: papers,
        }
    }

    fn candidate(id: &str) -> PurgeCandidate {
        PurgeCandidate {
            item_id: id.into(),
            terminal: true,
            leased: false,
            authoritative: false,
            source_receipt: true,
            decision_receipt: true,
            last_source_copy: false,
            active_work: false,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R826
    #[test]
    fn ties_break_by_goal_id_and_ineligible_purges_drop() {
        let goals = [goal("b", 10, 10), goal("a", 10, 10), goal("c", 30, 10)];
        let funded: Vec<&str> = allocate(&goals, 20)
            .iter()
            .map(|g| g.goal_id.as_str())
            .collect();
        assert_eq!(funded, ["a", "b"]);
        // A goal that does not fit is skipped, a cheaper later one still funds.
        let funded: Vec<&str> = allocate(&goals, 25)
            .iter()
            .map(|g| g.goal_id.as_str())
            .collect();
        assert_eq!(funded, ["a", "b"]);
        assert!(allocate(&goals, 5).is_empty());
        let mut leased = candidate("z");
        leased.leased = true;
        let mut last = candidate("y");
        last.last_source_copy = true;
        let mut active = candidate("x");
        active.active_work = true;
        let mut bare = candidate("w");
        bare.decision_receipt = false;
        let all = [candidate("m"), leased, last, active, bare, candidate("k")];
        let ids: Vec<&str> = purges(&all).iter().map(|c| c.item_id.as_str()).collect();
        assert_eq!(ids, ["k", "m"]);
    }
}
