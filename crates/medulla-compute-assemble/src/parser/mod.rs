// CONCEPT:MEDULLA-KG.compute.repository-parser — Repository Parser Module
//
// Feature-gated tree-sitter integration for source code parsing.
// Requires the `ast` feature flag.

// MEDULLA-SDLC-R014.1 — deterministic spec-artifact parser; no feature gate.
pub mod spec_artifacts;

#[cfg(feature = "ast")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod branch_scope;
/// The action a repository enrichment budget top-up must be verified for. The
/// durable store checks it in every build, so it lives outside the `ast` gate.
pub const TOP_UP_ACTION: &str = "repository:enrichment:budget:top_up";
#[cfg(feature = "ast")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod enrichment_snapshot;
#[cfg(feature = "ast")]
pub mod enrichment_work_machine;
