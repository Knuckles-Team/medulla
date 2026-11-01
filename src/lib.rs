#![allow(non_local_definitions)]
#![allow(dead_code)]
// The all-features dispatch future needs query depth 130. Use the same
// bounded compiler budget for production and test builds.
#![recursion_limit = "256"]
// No unsafe anywhere except a few platform/C-FFI boundaries, each carrying a
// scoped #[allow(unsafe_code)] with a soundness note: the tree-sitter block
// (medulla-compute ast/parser.rs), the CycloneDDS loan (server/dds.rs, ros2-rmw only),
// and the Win32 atomic-replace (server/reasoning_projection.rs, cfg(windows)).
// Any new unsafe is a compile error until it is explicitly justified the same way.
#![deny(unsafe_code)]

// CONCEPT:MEDULLA-KG.compute.graph-compute-engine - High-Performance Graph Compute Engine
// CONCEPT:MEDULLA-ORCH.execution.compiled-orchestration-kernel - Compiled Orchestration Kernel
// CONCEPT:MEDULLA-KG.query.tokio-service-server - Tokio Service Layer
//
// Tokio service layer handling MessagePack RPC.
// All logic delegated to graph, algorithms, and reasoning modules.
// Service-layer modules (protocol, registry, isolation, channels, server)
// are used by the medulla-server binary.

// Bottom-of-DAG wire types live in `medulla-types`; the graph core (storage, registry,
// isolation, semantic search) lives in `medulla-core`. Re-export both under the
// historical `crate::` paths so every module's `crate::protocol::` /
// `crate::graph::` / `crate::registry::` reference resolves unchanged.
pub use medulla_core::{compute, decay, graph, index, isolation, registry};
// CONCEPT:MEDULLA-KG.compute.message-broker-exchanges — message-broker primitives, re-exported under `crate::broker`.
#[cfg(feature = "broker")]
pub use medulla_core::broker;
pub use medulla_protocol_types::{change_envelope, mutation_batch, protocol};
pub use medulla_result_types::{epistemic_operations, epistemic_operations_ext};
#[cfg(feature = "knowledge-batch")]
pub use medulla_types::knowledge_stream;
pub use medulla_types::{acl, types, wire};
// Compute domains live in `medulla-compute`; re-export under the historical `crate::`
// paths. algorithms/ast/parser are always present; the feature-gated domains are
// re-exported only when their facade feature (→ medulla-compute feature) is on.
#[cfg(feature = "datascience")]
pub use medulla_compute::datascience;
#[cfg(feature = "finance")]
pub use medulla_compute::finance;
#[cfg(feature = "reasoning")]
pub use medulla_compute::reasoning;
pub use medulla_compute::{ast, parser, screen};
pub use medulla_graph_algos::algorithms;

// In-process embedded library API (CONCEPT:MEDULLA-KG.backend.engine-modes) — SQLite/DuckDB-style. Drives
// the SAME GraphCore + redb durable rows the socket dispatch does, with NO Tokio
// server/socket/HMAC. Gated on `embedded` (→ `redb`); needs NO `server` feature.
#[cfg(feature = "embedded")]
pub mod embedded;
// Shared test-only filesystem helpers. Keeping the process/epoch-nanos path
// construction in one owner prevents unit-test modules from drifting while
// preserving each caller's existing path prefix and cleanup contract.
#[cfg(feature = "server")]
pub mod server;

// Unified single binary — the opt-in in-process pyo3-engine (unified-binary program,
// workstream W-A; docs/architecture/unified-inprocess-engine.md). Re-exported ONLY
// under the opt-in `pyo3-engine` feature (never in default/full/cluster) so a
// Rust-only embedder can construct the SAME in-process engine handle the compiled
// `medulla.engine` extension wraps, with no Python involved. The
// out-of-process scale-out build links none of this crate and none of its optional
// dependencies (`scripts/check_no_pyo3.sh` scopes to exactly that build).
#[cfg(feature = "pyo3-engine")]
pub use medulla_pyengine;
