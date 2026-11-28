//! Width and depth rung choice, and the paired-comparison verdict record
//! (MEDULLA-DECISION-ENGINE-R134.12, R134.12.1).
//!
//! `Decide` with
//! [`QuestionKind::TrajectoryLadder`](super::statistical::QuestionKind::TrajectoryLadder)
//! picks one rung of the caller's ladder. A paired verdict is the stored
//! comparison of one rung's outcome against the baseline's on the same input
//! snapshot; it is written through `DecisionLog` only. Nothing here reads a
//! clock or a store.

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;
use medulla_types::refusal_code::RefusalCode;

/// Most rungs one ladder declares.
pub const MAX_RUNGS: usize = 64;

/// No rung satisfies `width <= max_width` and `depth <= trajectory.max_depth`.
pub const TRAJECTORY_NO_LEGAL_RUNG: &str = "TRAJECTORY_NO_LEGAL_RUNG";
/// The rung set lacks the baseline rung (`width 1, depth 1`).
pub const TRAJECTORY_BASELINE_MISSING: &str = "TRAJECTORY_BASELINE_MISSING";
/// A paired verdict lacks the outcome digest of one of its two arms.
pub const TRAJECTORY_ARM_MISSING: &str = "TRAJECTORY_ARM_MISSING";
/// A paired verdict's snapshot digest is not the decision's input snapshot digest.
pub const TRAJECTORY_SNAPSHOT_MISMATCH: &str = "TRAJECTORY_SNAPSHOT_MISMATCH";
/// A claim of improvement has no stored verdict whose winner is the arm.
pub const TRAJECTORY_IMPROVEMENT_UNPROVEN: &str = "TRAJECTORY_IMPROVEMENT_UNPROVEN";

/// A paired verdict names no trajectory-ladder decision of the caller's tenant.
pub const TRAJECTORY_DECISION_UNKNOWN: &str = "TRAJECTORY_DECISION_UNKNOWN";

/// Every refusal code of the trajectory-ladder family; joins
/// `code_registry::FAMILY_TABLES` under `QuestionKind::TrajectoryLadder`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(TRAJECTORY_NO_LEGAL_RUNG),
    RefusalCode::new(TRAJECTORY_BASELINE_MISSING),
    RefusalCode::new(TRAJECTORY_ARM_MISSING),
    RefusalCode::new(TRAJECTORY_SNAPSHOT_MISMATCH),
    RefusalCode::new(TRAJECTORY_IMPROVEMENT_UNPROVEN),
    RefusalCode::new(TRAJECTORY_DECISION_UNKNOWN),
];

/// One rung of the ladder: how many trajectories at what depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryRung {
    pub rung_id: String,
    pub width: u32,
    pub depth: u32,
}

impl TrajectoryRung {
    /// Whether this is the baseline rung, one trajectory of depth one.
    pub fn is_baseline(&self) -> bool {
        self.width == 1 && self.depth == 1
    }
}

/// The request of a `trajectory_ladder` question, carried as the JSON `ladder`
/// parameter. `improvement_claim` names a rung the caller claims improves on
/// the baseline; it is answered only with a stored verdict whose winner is the arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryLadderRequest {
    pub task_digest: String,
    pub input_snapshot_digest: String,
    /// The permitted trajectory-count upper bound (R140.100).
    pub max_width: u32,
    pub rungs: Vec<TrajectoryRung>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improvement_claim: Option<String>,
}

/// The rung the decision chose and the trajectories it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RungChoice {
    pub rung_id: String,
    pub width: u32,
    pub depth: u32,
    pub baseline_id: String,
    /// `<decision_digest_prefix>-t<k>` for `k` from 1 to `width`.
    pub trajectory_ids: Vec<String>,
    /// Whether a promoted head ranked the legal rungs.
    pub ranked_by_head: bool,
}

/// Which arm of a paired comparison won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Winner {
    Arm,
    Baseline,
    Tie,
}

/// One stored paired comparison of a rung against the baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairedVerdict {
    pub decision_id: String,
    pub rung_id: String,
    pub baseline_id: String,
    pub input_snapshot_digest: String,
    pub arm_outcome_digest: String,
    pub baseline_outcome_digest: String,
    pub winner: Winner,
}

/// The prefix every stored verdict key starts with.
pub const VERDICT_PREFIX: &str = "run_record/paired_verdict/";

impl PairedVerdict {
    /// The verdict's key in the tenant's decision artifact store.
    pub fn key(&self) -> String {
        format!("{VERDICT_PREFIX}{}/{}", self.decision_id, self.rung_id)
    }
}

/// The short digest prefix trajectory ids carry, derived from the request and
/// the chosen rung.
pub fn decision_digest_prefix(request: &TrajectoryLadderRequest, rung_id: &str) -> String {
    let digest = digest_text(
        "eg/trajectory-ladder/v1",
        &(
            &request.task_digest,
            &request.input_snapshot_digest,
            rung_id,
        ),
    );
    digest
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect()
}
