//! medulla-reasoner: OWL, tableau and rule reasoning plus reasoning closure.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (reasoning), durable-graph-kernel (forward chaining).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "reasoning")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning;
// CONCEPT:MEDULLA-KG.compute.reasoning-closure-gpu — semi-naive integer-interned rewrite of
// the `reasoning` fixpoint, with the transitive-closure join factored behind a
// `ClosureBackend` seam (CPU always-on + feature-gated CUDA kernel). Rides `reasoning`;
// the CUDA leg is further gated by `gpu-cuda`.
#[cfg(feature = "reasoning")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning_closure;
