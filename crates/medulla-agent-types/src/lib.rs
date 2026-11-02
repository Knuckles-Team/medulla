//! medulla-agent-types: Agent components, graphs, libraries, templates, work items, jobs and run history types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: orchestration-vocabulary (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// RF-020 — durable Agent Library identity and mutation-context contracts. The
// physical table and transaction adapter live above this pure-data crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_component;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_graph;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_guardrail;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_library;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_ontology;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_template;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_topology;
// RF-020 — typed Agent Library delegation admission/result currency.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod delegation;
// MEDULLA-DURABLE-KERNEL-R851.1 — the A2A peer record. Pure serde.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_peer;
// EH-408 / EH-409 — the receipt of a served telemetry derivation.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_fixtures;
