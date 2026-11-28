//! The Decide layer's wire contract (RF-ADR-010).
//!
//! Four methods share this vocabulary: `AgentAssemble` proves an agent graph
//! out of a tenant's own component library, `DecisionCommit` makes one of its
//! records durable, `Decide` answers a statistical question evaluate-only, and
//! `DecisionFit`/`DecisionEval` are the admin jobs that produce and qualify a
//! calibrated head.
//!
//! Three properties hold across all of them:
//!
//! * **A conclusion carries its premises.** Every record names the facts it
//!   used, the class of evidence each one is, and where it came from, so it is
//!   classified by its WEAKEST premise instead of by its own confidence.
//! * **Abstention is a first-class answer.** There is no arm that returns a
//!   guess shaped like a proof. An engine that cannot decide says what it could
//!   not resolve.
//! * **Nothing here is a float, and nothing reads a clock.** Times are recorded
//!   and never read by the math, so replaying a record anywhere reproduces its
//!   digest exactly.
//!
//! Every algorithm -- derivation, solving, verification, execution, fitting --
//! lives above this module. These are data types and their bounds.

#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod autonomy;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod branch_fusion;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod capability_rank;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod codes;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod command_config;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod comparative_analysis;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod defect_triage;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod delegation;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod digest;
pub mod entity_resolution;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod entry_types;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod errors;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod execution_part;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod feature_match;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod field_shielding;
pub mod ingest_shortfall;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod item_recommendation;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod knob_families;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod knobs;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod limit_types;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod memory_benchmark;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod memory_rewrite;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod model_capacity;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod numeric;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod paper_admission;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod path_success;
pub mod planner_cost_head;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod policy;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod process_plan;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod program_store;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod promotion_answer;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod promotion_tier;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod record;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod research_voi;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod resolve_conflict;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod reward;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod run_governance;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod safety;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod size;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod statistical;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod task_plan;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod thought_search;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod trajectory_verdict;
pub mod verifier_case_machine;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod verifier_receipt;

/// Format identity (RF-ADR-006) of an assembly decision record.
pub const DECISION_RECORD_SCHEMA_VERSION: u16 = 1;
/// Format identity of a statistical decision record. It starts at 2 because a
/// statistical record is the SECOND shape of the same durable record family,
/// not a second family.
pub const STATISTICAL_DECISION_RECORD_SCHEMA_VERSION: u16 = 2;
/// Format identity of an assembly record whose request asked for a topology
/// (SWARM-TOPOLOGY-DECIDE-DESIGN §6.5). A v1 record keeps its exact bytes and
/// digest: every topology field is absent from it, never serialized as null.
pub const TOPOLOGY_DECISION_RECORD_SCHEMA_VERSION: u16 = 3;
/// Format identity of a decision policy body.
pub const DECISION_POLICY_SCHEMA_VERSION: u16 = 1;
/// Format identity of a durable decision-job row.
pub const DECISION_JOB_SCHEMA_VERSION: u16 = 1;

/// Largest encoded decision record that may be committed.
pub const MAX_DECISION_RECORD_BYTES: usize = 64 * 1024;
/// Largest candidate set one assembly may consider.
pub const MAX_ASSEMBLY_CANDIDATES: usize = 64;
/// Largest required-capability set one assembly request may carry.
pub const MAX_ASSEMBLY_REQUIRED_CAPABILITIES: usize = 32;
/// Largest number of graph templates one assembly may enumerate.
pub const MAX_ASSEMBLY_TEMPLATES: usize = 8;
/// Largest number of slots one template may declare.
pub const MAX_TEMPLATE_SLOTS: usize = 6;

pub use errors::DecisionErrorCode;

pub use numeric::{
    QuantScaleTag, QuantisedValue, UnitRationalFields, UnitRationalWire,
    MAX_UNIT_RATIONAL_DENOMINATOR,
};
pub use policy::{
    ColdStart, DecisionPolicy, ExplorationBudget, ObjectiveLevelKind, ObjectiveOrder,
    StatisticalPolicy, TraceFidelityLevel, UnknownCostRule, WeightedLevel,
};
pub use {
    crate::decision::record::premise::CoverageDerivation,
    crate::decision::record::premise::DerivationEdge, crate::decision::record::premise::EdgeSource,
    crate::decision::record::premise::Elimination, crate::decision::record::premise::PremiseClass,
    crate::decision::record::premise::PremiseProvenance,
    crate::decision::record::premise::PremiseRef, crate::decision::record::premise::Violation,
};

pub use crate::decision::statistical::question_kind::QuestionKind;

/// The columns of the `decisions` relation a caller's decision log exposes, in order
/// (MEDULLA-FEDERATED-QUERY-R058.4.7.2.1). `medulla-h-decide` builds the relation from its
/// own column table and a unit test there pins the two lists equal, so natural-language
/// grounding of `DECISIONS WHERE <column>` reads the schema the log actually serves.
pub const DECISION_LOG_COLUMNS: &[&str] = &[
    "record_id",
    "question_id",
    "question_kind",
    "safety",
    "source",
    "outcome",
    "option_id",
    "resolution_kind",
    "evidence_class",
    "policy_digest",
    "committed_by",
    "created_at_ms",
    "committed_at_ms",
];
