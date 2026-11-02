//! medulla-persistence-records: Persisted agent, decision, connector pack and write-back records.
//!
//! Level F6 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// The three-layer native agent hierarchy (RF-ADR-008) is a set of durable redb
// owners; every consumer is itself `redb`-gated.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_component;
#[cfg(all(any(test, feature = "test-support"), feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_fixtures;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_graph;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_library;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_pin_resolution;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_revision;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_row;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_template;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod connector_pack;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision_jobs;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision_record;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod guardrail_rule;
// EH-345: the fleet catalog's one read over the AgentComponent owner.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fleet_components;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod write_back;
