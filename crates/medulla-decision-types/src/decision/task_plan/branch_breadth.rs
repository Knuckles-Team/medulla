//! Branch-start experience retrieval and outcome-driven breadth
//! (MEDULLA-DECISION-ENGINE-R140.102).
//!
//! A branch with no history starts at `breadth.initial`. A branch with history
//! moves the previous breadth by the deltas of its latest attempts and clamps
//! the result. The history is checked against the stored branch outcomes the
//! caller read for its own tenant; a missing outcome is never read as a zero
//! outcome.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;

use super::trajectory_refusal::{request_premise, TrajectoryRefusal};
use crate::decision::knob_families::trajectory as knobs;
use crate::decision::policy::DecisionPolicy;
use crate::decision::record::outcome::RefusalCode;
use crate::decision::{PremiseClass, PremiseRef};

/// A history ref names an outcome at a revision or digest the store has moved past.
pub const BREADTH_HISTORY_STALE: RefusalCode = RefusalCode::new("BREADTH_HISTORY_STALE");
/// A present history is empty or names an outcome the store lacks.
pub const BREADTH_HISTORY_INCOMPLETE: RefusalCode = RefusalCode::new("BREADTH_HISTORY_INCOMPLETE");
/// A history ref names an outcome of another tenant.
pub const BREADTH_HISTORY_FORBIDDEN: RefusalCode = RefusalCode::new("BREADTH_HISTORY_FORBIDDEN");

/// The refusal codes of the breadth decision.
pub const REFUSALS: &[RefusalCode] = &[
    BREADTH_HISTORY_STALE,
    BREADTH_HISTORY_INCOMPLETE,
    BREADTH_HISTORY_FORBIDDEN,
];

const BREADTH_DIGEST_DOMAIN: &str = "eg/branch-breadth/v1";
const HISTORY_DIGEST_DOMAIN: &str = "eg/branch-breadth-history/v1";

/// How a past branch attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResultClass {
    Success,
    Partial,
    Failure,
}

/// One past attempt the caller cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OutcomeRef {
    pub branch_id: String,
    pub outcome_revision: u64,
    pub outcome_digest: String,
    pub result_class: ResultClass,
}

/// The history a branch starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "history", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BranchHistory {
    Absent,
    Present {
        /// The breadth the previous attempt ran with.
        previous_breadth: u32,
        /// The attempts in order, oldest first.
        refs: Vec<OutcomeRef>,
    },
}

/// The breadth question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BreadthRequest {
    pub search_id: String,
    pub branch_id: String,
    pub ordinal: u32,
    pub plan_digest: String,
    pub history: BranchHistory,
}

/// A branch outcome as the store holds it for `tenant_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBranchOutcome {
    pub tenant_id: String,
    pub outcome_revision: u64,
    pub outcome_digest: String,
    pub result_class: ResultClass,
}

/// The decided breadth and what it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BreadthDecision {
    pub search_id: String,
    pub branch_id: String,
    pub ordinal: u32,
    pub plan_digest: String,
    pub history_digest: String,
    /// Zero when the branch has no history.
    pub previous_breadth: u32,
    pub selected_breadth: u32,
    pub decision_digest: String,
}

/// The knobs the decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreadthKnobs {
    pub initial: i64,
    pub window: usize,
    pub max: i64,
    pub delta_success: i64,
    pub delta_partial: i64,
    pub delta_failure: i64,
}

impl BreadthKnobs {
    /// The knobs a pinned policy sets, each held inside its declared range.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        Self {
            initial: knobs::read(policy, &knobs::BREADTH_INITIAL),
            window: knobs::read(policy, &knobs::BREADTH_WINDOW) as usize,
            max: knobs::read(policy, &knobs::BREADTH_MAX),
            delta_success: knobs::read(policy, &knobs::BREADTH_DELTA_SUCCESS),
            delta_partial: knobs::read(policy, &knobs::BREADTH_DELTA_PARTIAL),
            delta_failure: knobs::read(policy, &knobs::BREADTH_DELTA_FAILURE),
        }
    }

    fn delta(&self, class: ResultClass) -> i64 {
        match class {
            ResultClass::Success => self.delta_success,
            ResultClass::Partial => self.delta_partial,
            ResultClass::Failure => self.delta_failure,
        }
    }
}

impl Default for BreadthKnobs {
    fn default() -> Self {
        Self {
            initial: 2,
            window: 3,
            max: 8,
            delta_success: -1,
            delta_partial: 0,
            delta_failure: 1,
        }
    }
}

fn refuse(code: RefusalCode, detail: impl Into<String>) -> TrajectoryRefusal {
    TrajectoryRefusal::new(code, detail)
}

/// Decide the breadth of a branch for `tenant_id`, reading `stored` (keyed by
/// outcome branch id) for every cited attempt. The initial breadth is clamped
/// to `[1, breadth.max]` like every other result.
pub fn decide_breadth(
    request: &BreadthRequest,
    tenant_id: &str,
    stored: &BTreeMap<String, StoredBranchOutcome>,
    knobs: &BreadthKnobs,
) -> Result<(BreadthDecision, Vec<PremiseRef>), TrajectoryRefusal> {
    let clamp = |value: i64| value.clamp(1, knobs.max.max(1)) as u32;
    let (previous, selected, history_digest, class) = match &request.history {
        BranchHistory::Absent => (
            0,
            clamp(knobs.initial),
            digest_text(HISTORY_DIGEST_DOMAIN, &Vec::<&OutcomeRef>::new()),
            PremiseClass::Claim,
        ),
        BranchHistory::Present {
            previous_breadth,
            refs,
        } => {
            if refs.is_empty() {
                return Err(refuse(
                    BREADTH_HISTORY_INCOMPLETE,
                    "a present history cites at least one attempt",
                ));
            }
            let mut classes = Vec::with_capacity(refs.len());
            for cited in refs {
                let Some(record) = stored.get(&cited.branch_id) else {
                    return Err(refuse(
                        BREADTH_HISTORY_INCOMPLETE,
                        format!("no stored outcome for branch {}", cited.branch_id),
                    ));
                };
                if record.tenant_id != tenant_id {
                    return Err(refuse(
                        BREADTH_HISTORY_FORBIDDEN,
                        format!(
                            "outcome of branch {} is outside the caller tenant",
                            cited.branch_id
                        ),
                    ));
                }
                if record.outcome_revision != cited.outcome_revision
                    || record.outcome_digest != cited.outcome_digest
                    || record.result_class != cited.result_class
                {
                    return Err(refuse(
                        BREADTH_HISTORY_STALE,
                        format!(
                            "branch {} is stored at revision {}",
                            cited.branch_id, record.outcome_revision
                        ),
                    ));
                }
                classes.push(record.result_class);
            }
            let skip = classes.len().saturating_sub(knobs.window);
            let moved: i64 = classes[skip..].iter().map(|c| knobs.delta(*c)).sum();
            (
                *previous_breadth,
                clamp(i64::from(*previous_breadth) + moved),
                digest_text(HISTORY_DIGEST_DOMAIN, refs),
                PremiseClass::Observation,
            )
        }
    };
    let mut decision = BreadthDecision {
        search_id: request.search_id.clone(),
        branch_id: request.branch_id.clone(),
        ordinal: request.ordinal,
        plan_digest: request.plan_digest.clone(),
        history_digest,
        previous_breadth: previous,
        selected_breadth: selected,
        decision_digest: String::new(),
    };
    decision.decision_digest = digest_text(
        BREADTH_DIGEST_DOMAIN,
        &(
            &decision.history_digest,
            decision.previous_breadth,
            decision.selected_breadth,
            &decision.plan_digest,
        ),
    );
    let premises = vec![
        request_premise(
            "breadth:history",
            format!("breadth:history={}", decision.history_digest),
            class,
            "params.breadth.history",
        ),
        request_premise(
            "breadth:previous",
            format!("breadth:previous={}", decision.previous_breadth),
            class,
            "params.breadth.history",
        ),
        request_premise(
            "breadth:selected",
            format!(
                "breadth:selected={};plan_digest={}",
                decision.selected_breadth, decision.plan_digest
            ),
            class,
            "params.breadth.plan_digest",
        ),
    ];
    Ok((decision, premises))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TENANT: &str = "tenant-a";

    fn cited(branch: &str, class: ResultClass) -> OutcomeRef {
        OutcomeRef {
            branch_id: branch.into(),
            outcome_revision: 2,
            outcome_digest: format!("sha256:{branch}"),
            result_class: class,
        }
    }

    fn stored(refs: &[OutcomeRef]) -> BTreeMap<String, StoredBranchOutcome> {
        refs.iter()
            .map(|r| {
                (
                    r.branch_id.clone(),
                    StoredBranchOutcome {
                        tenant_id: TENANT.into(),
                        outcome_revision: r.outcome_revision,
                        outcome_digest: r.outcome_digest.clone(),
                        result_class: r.result_class,
                    },
                )
            })
            .collect()
    }

    fn request(history: BranchHistory) -> BreadthRequest {
        BreadthRequest {
            search_id: "search".into(),
            branch_id: "next".into(),
            ordinal: 4,
            plan_digest: "sha256:plan".into(),
            history,
        }
    }

    fn present(previous: u32, refs: Vec<OutcomeRef>) -> BranchHistory {
        BranchHistory::Present {
            previous_breadth: previous,
            refs,
        }
    }

    fn selected(history: BranchHistory, refs: &[OutcomeRef], knobs: &BreadthKnobs) -> u32 {
        decide_breadth(&request(history), TENANT, &stored(refs), knobs)
            .unwrap()
            .0
            .selected_breadth
    }

    // spec: MEDULLA-DECISION-ENGINE-R140.102
    #[test]
    fn mapping_and_clamp() {
        let knobs = BreadthKnobs::default();
        // No history: the declared initial breadth, previous breadth zero.
        let (absent, premises) = decide_breadth(
            &request(BranchHistory::Absent),
            TENANT,
            &BTreeMap::new(),
            &knobs,
        )
        .unwrap();
        assert_eq!((absent.previous_breadth, absent.selected_breadth), (0, 2));
        assert_eq!(premises[0].class, PremiseClass::Claim);
        // Each result class moves the previous breadth by its delta.
        for (class, expected) in [
            (ResultClass::Success, 4),
            (ResultClass::Partial, 5),
            (ResultClass::Failure, 6),
        ] {
            let refs = [cited("x", class)];
            assert_eq!(selected(present(5, refs.to_vec()), &refs, &knobs), expected);
        }
        // Only the last `window` attempts count: oldest failures fall out of a window of 2.
        let refs = [
            cited("a", ResultClass::Failure),
            cited("b", ResultClass::Failure),
            cited("c", ResultClass::Success),
            cited("d", ResultClass::Success),
        ];
        let two = BreadthKnobs { window: 2, ..knobs };
        assert_eq!(selected(present(5, refs.to_vec()), &refs, &two), 3);
        assert_eq!(selected(present(5, refs.to_vec()), &refs, &knobs), 4);
        // Both clamps.
        let wins: Vec<OutcomeRef> = ["a", "b", "c"]
            .iter()
            .map(|b| cited(b, ResultClass::Success))
            .collect();
        assert_eq!(selected(present(2, wins.clone()), &wins, &knobs), 1);
        let losses: Vec<OutcomeRef> = ["a", "b", "c"]
            .iter()
            .map(|b| cited(b, ResultClass::Failure))
            .collect();
        assert_eq!(selected(present(7, losses.clone()), &losses, &knobs), 8);
        assert_eq!(
            selected(
                BranchHistory::Absent,
                &[],
                &BreadthKnobs {
                    initial: 12,
                    ..knobs
                }
            ),
            8
        );
        // The decision digest covers history, previous, selected and plan digests.
        let refs = [cited("x", ResultClass::Failure)];
        let base = decide_breadth(
            &request(present(5, refs.to_vec())),
            TENANT,
            &stored(&refs),
            &knobs,
        )
        .unwrap();
        assert_eq!(base.1[0].class, PremiseClass::Observation);
        let mut other_plan = request(present(5, refs.to_vec()));
        other_plan.plan_digest = "sha256:other".into();
        let moved = decide_breadth(&other_plan, TENANT, &stored(&refs), &knobs).unwrap();
        assert_ne!(base.0.decision_digest, moved.0.decision_digest);
        // Refusals.
        let code = |history: BranchHistory,
                    store: &BTreeMap<String, StoredBranchOutcome>,
                    tenant: &str| {
            decide_breadth(&request(history), tenant, store, &knobs)
                .unwrap_err()
                .code
        };
        let full = stored(&refs);
        assert_eq!(
            code(present(5, vec![]), &full, TENANT),
            BREADTH_HISTORY_INCOMPLETE
        );
        assert_eq!(
            code(present(5, refs.to_vec()), &BTreeMap::new(), TENANT),
            BREADTH_HISTORY_INCOMPLETE
        );
        assert_eq!(
            code(present(5, refs.to_vec()), &full, "tenant-b"),
            BREADTH_HISTORY_FORBIDDEN
        );
        let mut newer = full.clone();
        newer.get_mut("x").unwrap().outcome_revision = 3;
        assert_eq!(
            code(present(5, refs.to_vec()), &newer, TENANT),
            BREADTH_HISTORY_STALE
        );
        let mut changed = full.clone();
        changed.get_mut("x").unwrap().outcome_digest = "sha256:moved".into();
        assert_eq!(
            code(present(5, refs.to_vec()), &changed, TENANT),
            BREADTH_HISTORY_STALE
        );
    }
}
