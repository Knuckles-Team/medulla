#![allow(non_local_definitions)]
#![allow(dead_code)]
// The all-features dispatch future needs query depth 130. Use the same
// bounded compiler budget for production and test builds.
#![recursion_limit = "256"]
// No unsafe anywhere except a few platform/C-FFI boundaries, each carrying a
// scoped #[allow(unsafe_code)] with a soundness note. Any new unsafe is a compile
// error until it is explicitly justified the same way.
#![deny(unsafe_code)]

// CONCEPT:MEDULLA-KG.compute.graph-compute-engine - High-Performance Graph Compute Engine
// CONCEPT:MEDULLA-ORCH.execution.compiled-orchestration-kernel - Compiled Orchestration Kernel
// CONCEPT:MEDULLA-KG.query.tokio-service-server - Tokio Service Layer
//
// The facade crate composes the engine crates: it owns the in-process embedded API, the
// binaries and the opt-in pyo3 bindings. Every other module lives in its owning crate
// (`medulla-core`, `medulla-server`, ...) and is reached by that crate's path.

// In-process embedded library API (CONCEPT:MEDULLA-KG.backend.engine-modes) — SQLite/DuckDB-style. Drives
// the SAME GraphCore + redb durable rows the socket dispatch does, with NO Tokio
// server/socket/HMAC. Gated on `embedded` (→ `redb`); needs NO `server` feature.
#[cfg(feature = "embedded")]
pub mod embedded;

// Unified single binary — the opt-in in-process pyo3-engine (unified-binary program,
// workstream W-A; docs/architecture/unified-inprocess-engine.md). Re-exported ONLY
// under the opt-in `pyo3-engine` feature (never in default/full/cluster) so a
// Rust-only embedder can construct the SAME in-process engine handle the compiled
// `medulla.engine` extension wraps, with no Python involved.
#[cfg(feature = "pyo3-engine")]
pub use medulla_pyengine;
