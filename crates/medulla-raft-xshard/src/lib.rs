//! medulla-raft-xshard: Cross-shard transactions and reads, elasticity and the raft test harnesses.
//!
//! Level F8 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (cross-shard).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(feature = "raft")]

/// Pure, policy-only M3 cross-node elasticity proposals with resumable move identities.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cross_node_elasticity;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cross_shard_txn;

/// Bounded shard/Raft drain state machine (NE-167): admission is stopped and
/// observed before a voter is removed, every acknowledgement is revision/fence
/// bound, and restart recovery fails closed.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod drain;

/// X4 — the distributed cross-shard DAG EXCHANGE operator (CONCEPT:MEDULLA-KG.query.dag-distributed-exchange),
/// the cluster-tier extension of E5's `medulla_plan::dag::PlanDag`: ships a branch subtree to
/// its owning Raft group over a length-prefixed-MessagePack transport, runs it there via
/// the unmodified `medulla_plan::execute_dag`, and merges the partials back through
/// `medulla_plan::execute_dag_with`'s unmodified multi-branch join. Additionally gated on
/// `query` (needs `medulla_plan`'s `PlanCtx`/`dag_exec`, which a plain `raft`-without-`query`
/// build does not link) — `cluster` implies both.
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod exchange;

/// Typed flake-budget / retry-policy model for running cluster tests reliably
/// under representative load (MEDULLA-DURABLE-KERNEL-R014.1): bounds how many
/// transient failures a run under load may absorb and at what backoff,
/// refusing a malformed budget rather than silently retrying forever or not
/// at all. The load-test harness and representative-load generator that
/// consume it are a later child.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod flake_budget;

/// Distributed graph compute — the Pregel/GAS cross-shard superstep engine
/// (CONCEPT:MEDULLA-KG.storage.feature). Behind `compute-dist` (which implies `raft`): runs PageRank /
/// connected-components / BFS across graphs spanning multiple Raft groups, plus the
/// incremental/streaming variant and materialized views.
#[cfg(feature = "compute-dist")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pregel;

/// Shared setup helpers for in-process Raft harnesses.
#[cfg(any(test, feature = "harness", feature = "compute-dist", feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod harness_support;

/// The in-process Raft cluster fixture. It is shared by the `harness` gauntlets
/// (through `harness::cluster::fixture`) and the `compute-dist` modality harness,
/// so it is declared once, here, under the union of their gates. Loading the file
/// from both places compiled it twice (`clippy::duplicate_mod`).
#[cfg(any(test, feature = "harness", feature = "compute-dist"))]
#[path = "harness/fixture.rs"]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fixture;

/// Correctness + load harness (CONCEPT:AU-KG.ontology.emits-database-ontology-entities) — the standing proof-engine that
/// gates every distributed/durability claim. Compiled under tests OR the explicit
/// `harness` feature; never in a production tier build.
#[cfg(any(test, feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod harness;

// The online-resharding + tenant-hibernation gauntlet (CONCEPT:MEDULLA-KG.storage.100m-tenant) — proves a
// graph reshareded A→B keeps all data + serves correctly, and a hibernated graph
// rehydrates intact. Gated behind `harness` so a normal `raft` build links nothing.
#[cfg(all(test, feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod reshard_harness;

// The cross-shard READ gauntlet (CONCEPT:MEDULLA-KG.sharding.placement-catalog, DIST-P2-2) — proves a
// read spanning two groups gathers + union-merges both, each leg routes via the
// PlacementCatalog, a single-group read is not flagged cross-shard, and an unreachable
// leg errors loudly. Gated behind `harness` so a normal `raft` build links nothing.
#[cfg(all(test, feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod xread_harness;

/// In-process cross-shard 2PC **modality-spanning** coordinator-kill harness
/// (CONCEPT:MEDULLA-KG.txn.crossshard-2pc-modality-harness) — the `--features cluster`
/// proof that closes MEDULLA-396: a cross-shard txn spanning the property-graph + RDF
/// modalities across two Raft groups stays all-or-nothing when the coordinator is
/// killed mid-2PC (recovery resolves to a SINGLE decision). Gated on `compute-dist`
/// (which `cluster` implies) so the pub proof entry is reachable from an external
/// `--features cluster` integration test, and its `#[cfg(test)]` scenarios ride the
/// `cargo test --features cluster` gate.
#[cfg(feature = "compute-dist")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod xshard_modality_harness;
