//! Deterministic business rule kinds (MEDULLA-BUSINESS-OPS-R002).
//!
//! Five rule kinds evaluate in the engine: recurrence, threshold, reconcile, reorder
//! and phrase policy. Every evaluation takes its as-of time as an explicit argument,
//! uses integer arithmetic only and returns the same value for the same inputs on every
//! host. Every result carries a [`rule_spec::RuleTrace`]: the digest of the rule that
//! produced it, the input nodes with their revisions and the steps taken.

pub mod phrase_policy;
pub mod reconcile;
pub mod recurrence;
pub mod registry;
pub mod reorder;
pub mod rule_spec;
pub mod threshold;

/// Every refusal code an evaluation of any rule kind can raise: the codes of the rule
/// record and of each of the five kinds. The `EVALUATE RULE` stage answers a refusal as
/// `<CODE>: <detail>`, and a code outside this list is a defect.
pub const RULE_REFUSAL_CODES: &[&str] = &[
    rule_spec::ASOF_REQUIRED,
    rule_spec::RULE_NOT_FOUND,
    rule_spec::RULE_PARAMETERS_INVALID,
    rule_spec::RULE_DIGEST_MISMATCH,
    rule_spec::RULE_INPUT_INVALID,
    rule_spec::RULE_KIND_UNKNOWN,
    rule_spec::RULE_VERSION_UNKNOWN,
    recurrence::RECURRENCE_COUNT_INVALID,
    recurrence::RECURRENCE_RULE_INVALID,
    recurrence::RECURRENCE_HORIZON_EXCEEDED,
    threshold::THRESHOLD_METRIC_UNDEFINED,
    threshold::THRESHOLD_EXPRESSION_INVALID,
    threshold::THRESHOLD_OVERFLOW,
    reconcile::RECONCILE_INPUT_TOO_LARGE,
    reconcile::RECONCILE_WORK_EXCEEDED,
    reconcile::RECONCILE_PARAMS_INVALID,
    reorder::REORDER_POLICY_INVALID,
    reorder::REORDER_OVERFLOW,
    phrase_policy::PHRASE_TEXT_TOO_LARGE,
    phrase_policy::PHRASE_POLICY_INVALID,
    phrase_policy::PHRASE_MODE_INVALID,
];
