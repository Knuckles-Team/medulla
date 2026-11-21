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
pub mod reorder;
pub mod rule_spec;
pub mod threshold;
