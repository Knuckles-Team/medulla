//! medulla-sql-udfs: SQL scalar, numeric, sketch, window and table functions registered into the SQL engine.
//!
//! Level L4c in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// CONCEPT:MEDULLA-KG.query.surface-b-numeric-operators — Analytics-Program Surface-B numeric SQL operators (`cosine_sim`,
/// `l2_normalize`, `zscore`, `covariance`) backed by the `medulla-numeric` kernel. Feature
/// `numeric` (out of `pi`); no pyo3 in the engine build.
#[cfg(feature = "numeric")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod numeric;
/// EH-522 — `eg_<func>(…) OVER (…)` window functions over the series kernels.
#[cfg(feature = "numeric")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod series_udwf;
/// Sketch-backed SQL aggregate UDFs (CONCEPT:MEDULLA-KG.query.approx-distinct-cardinality, W4.5/N5):
/// `approx_distinct`/`approx_frequency`/`minhash_signature` + the `minhash_similarity` scalar.
/// Always-on under base `sql` — the underlying sketches (`medulla_compute::sketch`) are pure-Rust
/// with zero heavy deps, already-linked `medulla-compute` unconditionally carries them.
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sketch_udfs;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod tablefuncs;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod udfs;
