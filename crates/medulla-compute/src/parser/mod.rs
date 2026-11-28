#[cfg(feature = "ast")]
pub mod tree_sitter;
// CONCEPT:MEDULLA-KG.compute.turn-each-project — cross-file call/import resolution over a parsed batch.
#[cfg(feature = "ast")]
pub mod resolve;
// CONCEPT:EH-280 — branch-aware, blob-deduplicated indexing: every unique blob is
// parsed once and projected as `:Blob` / `:FileVersion` / `:Branch` membership.
#[cfg(feature = "ast")]
pub mod branch_index;
#[cfg(feature = "ast")]
mod branch_projection;
#[cfg(feature = "ast")]
pub mod enrichment_admission;
#[cfg(feature = "ast")]
pub mod enrichment_machine;
#[cfg(feature = "ast")]
pub mod enrichment_reactivation;
