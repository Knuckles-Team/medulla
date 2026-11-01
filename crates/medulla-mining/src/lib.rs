//! medulla-mining: Pattern and process mining over graph records.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// CONCEPT:MEDULLA-KG.mining.frequent-itemset-mining — descriptive data-mining domain
// (frequent itemsets + association rules). Pure-Rust, dependency-light, batch;
// graph-agnostic (works over interned item ids), so it is unit-testable in
// isolation. Feature-gated like finance/datascience so a slim build drops it.
#[cfg(feature = "mining")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mining;
