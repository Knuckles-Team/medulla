//! medulla-agent-types: Agent components, graphs, libraries, templates, work items, jobs and run history types.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: orchestration-vocabulary (types).

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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod concept_lineage;
// RF-020 — typed Agent Library delegation admission/result currency.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod delegation;
// EH-408 / EH-409 — the receipt of a served telemetry derivation.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_fixtures;
