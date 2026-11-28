//! The branch-merge conflict winner question (MEDULLA-DECISION-ENGINE-R828).
//!
//! `Decide` with
//! [`QuestionKind::ResolveConflict`](super::statistical::QuestionKind::ResolveConflict)
//! scores both candidates of a conflict record as `source_reliability x
//! confidence`. The higher score wins when the gap is at least `knob
//! conflict.min_margin`; a smaller gap may break the tie by the later
//! `valid_from` only when the validity windows overlap and `knob
//! conflict.recency_tiebreak` is 1. Otherwise the decision abstains. The
//! decision writes nothing: `BranchMerge` writes the resolution.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use crate::decision::record::outcome::refusal::DecisionRefusal;
use medulla_types::refusal_code::RefusalCode;

/// Neither candidate wins by the margin, and no recency tie-break applies.
pub const CONFLICT_UNRESOLVED: &str = "CONFLICT_UNRESOLVED";
/// The request names no pinned decision policy.
pub const CONFLICT_POLICY_MISSING: &str = "CONFLICT_POLICY_MISSING";
/// A candidate's score is not a finite number.
pub const CONFLICT_NONFINITE: &str = "CONFLICT_NONFINITE";

/// The id of the `resolve_conflict` question family in the decide family
/// registry (MEDULLA-DECISION-ENGINE-R001.101). It is the one source of the
/// conflict family's published schema component id.
pub const CONFLICT_FAMILY_ID: &str = "resolve_conflict";
/// The component id of the `FeatureSchema` the conflict family publishes: its
/// family id, so a pin is resolved by the family id and never chosen by a
/// caller (MEDULLA-BRANCH-MERGE-R007).
pub fn conflict_feature_schema_component_id() -> String {
    format!("{CONFLICT_FAMILY_ID}.feature_schema")
}
/// The conflict family has published no feature schema in this tenant.
pub const CONFLICT_SCHEMA_MISSING: &str = "CONFLICT_SCHEMA_MISSING";

/// The one option a conflict question declares.
pub const CONFLICT_OPTION: &str = "resolve_conflict";
/// The request parameter carrying the `ConflictCandidates`.
pub const PARAM_CONFLICT: &str = "conflict";
/// The answer naming candidate A the winner.
pub const WINNER_A: &str = "candidate_a";
/// The answer naming candidate B the winner.
pub const WINNER_B: &str = "candidate_b";

/// Every refusal code of the family; joins `code_registry::FAMILY_TABLES` under
/// `QuestionKind::ResolveConflict`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(CONFLICT_UNRESOLVED),
    RefusalCode::new(CONFLICT_POLICY_MISSING),
    RefusalCode::new(CONFLICT_NONFINITE),
];

/// The `conflict.*` knobs: the winning score margin (a ratio in millionths)
/// and whether a later `valid_from` breaks a tie (0 or 1).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "conflict.min_margin",
        unit: "ratio",
        default: KnobValue::DecimalMicros(150_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "conflict.recency_tiebreak",
        unit: "flag",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Tunable,
    },
];

fn micros(value: KnobValue) -> i64 {
    match value {
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
        KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
    }
}

/// The resolved `conflict.min_margin` as a ratio, held inside `[0, 1]`.
pub fn min_margin(policy: &DecisionPolicy) -> f64 {
    let value = micros(policy.knob(&KNOBS[0])).clamp(0, 1_000_000);
    value as f64 / 1_000_000.0
}

/// Whether `conflict.recency_tiebreak` is on.
pub fn recency_tiebreak(policy: &DecisionPolicy) -> bool {
    micros(policy.knob(&KNOBS[1])) == 1
}

/// Why a conflict decision abstains: one registered code and its detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl ConflictRefusal {
    /// A refusal under `code`, which must be one of [`REFUSALS`].
    pub fn new(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            code: RefusalCode::new(code),
            detail: detail.to_string(),
        }
    }
}

impl DecisionRefusal for ConflictRefusal {
    fn code(&self) -> RefusalCode {
        self.code
    }

    fn detail(&self) -> String {
        self.detail.clone()
    }
}
