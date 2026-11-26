//! medulla-mutation-types: Mutation, batch, change envelope, commit descriptor, outbox, lease and consensus record types.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-DURABLE-KERNEL-R042 — a published benchmark report's typed superiority
// claim and the refusal for an unpublished-configuration or non-passing one.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod benchmark_claim;
// MEDULLA-BRANCH-MERGE-R001 — typed branch registry row and name rule.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod branch;
// CONCEPT:AU-ORCH.scheduling — GOC-21 distributed CapacityCell/CapacityLease
// wire contract + pure fencing/admission algorithm (cross-host capacity
// authority for the AU fair scheduler). Unconditional, like `acl`/`jobs`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_lease;
// GOC-03 — the cross-domain commit-descriptor/read-barrier currency shared by
// graph, modality, vector, blob/refcount, time-series, evidence, table/lake, and
// terminal-analytics-outcome participants. Deliberately a NEW module (not folded
// into `mutation_batch`): `MutationBatch`/`MutationProjectionCursor` remain the
// per-surface request/outbox envelope; `CommitDescriptor` is the one commit
// identity every domain's participant registers against. See
// `plans/graph-os-completion-program/lanes/GOC-03-cross-domain-commit-currency.md`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod commit_descriptor;
// RF-RULING-004 — current-only parent/participant transaction coordination DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod consensus;
// MEDULLA-DURABLE-KERNEL-R034 — cross-store atomicity's typed transaction-scope
// model: a single-store scope structurally can never map to the cross-store
// commit path. Pure serde, no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cross_store_scope;
// MEDULLA-DURABLE-KERNEL-R032 — the native primary-key fast-path routing decision
// (pgwire/MySQL/MSSQL point operations that bypass DataFusion planning).
// Pure serde, no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fast_path;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_outbox;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_scope;
// RF-RULING-004 — mutation-kernel-owned outbox intent, delivery, and cursor DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod outbox;
// MEDULLA-DURABLE-KERNEL-R874.13 -- the outbox-delivery state machine.
pub mod outbox_delivery_machine;
// MEDULLA-DURABLE-KERNEL-R874.14, R874.15 -- the parent and participant transaction machines.
pub mod parent_transaction_machine;
pub mod participant_transaction_machine;
// MEDULLA-DURABLE-KERNEL-R035 — the PostgreSQL wire protocol's required
// operational surfaces and a fail-closed compatibility matrix. Pure serde,
// no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pg_operational_surface;
/// The logical-replication slot row and its reserved namespace (MEDULLA-DURABLE-KERNEL-R035.7.1).
pub mod pg_replication;
pub mod sql_form_machine;
// MEDULLA-DURABLE-KERNEL-R040 — pipelined RESP/pgwire-extended batch admission:
// one admission maps to exactly one commit with order-preserving replies.
// Pure serde, no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pipeline_batch;
// MEDULLA-DURABLE-KERNEL-R700.1 — the retention-cleanup policy shape.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retention_cleanup;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retention_policy;
// EH-558 — sealed record classes: content-addressed rows generic writes may not change.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sealed_record;
// MEDULLA-BRANCH-MERGE-R006 — transaction, commit and diff vocabulary of branch time travel.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod versioning;
// MEDULLA-DURABLE-KERNEL-R037 — the workload classification (point/structure/sql)
// every wire listener routes an operation by. Pure, no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod workload_class;
// EH-560 -- governed changes: reserved control-lease kinds whose approval EG
// proves two-person (approver != proposer, exact digest, single use).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod governed_change;
// MEDULLA-DURABLE-KERNEL-R043.100.1 — graph change-feed types and snapshot digest.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_change_feed;
// MEDULLA-ORCH-VOCAB-R010.100 — one proposal's measured source traces as a read view.
pub mod evolution_metrics;
// MEDULLA-ORCH-VOCAB-R010.1 — the closed harness metric statement table, beside
// the price table its usage statement reads.
pub mod harness_metrics;
pub mod harness_session_search;
// MEDULLA-DURABLE-KERNEL-R852.1/.2/.4 — health anomalies and correlated incidents.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod health_incident;
pub mod incident_machine;
// MEDULLA-DURABLE-KERNEL-R033 — the hot-store engine evaluation's typed model
// (redb/fjall/RocksDB candidates, workload profiles, validated benchmark
// results, winner selection). Pure serde, no dep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod hot_store_engine;
// MEDULLA-DURABLE-KERNEL-R085.1 — memory snapshot head, sync request/outcome, cursor and
// the pure compare-and-set core. Pure serde; served handlers live above this crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod memory_sync;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mirror_sink;
// EH-406 -- error-budget adaptive (AIMD) throttling on a capacity cell:
// automatic steps only narrow or give back what they narrowed.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_throttle;
// RF-ADR-009 D18 — EG-owned versioned source change sets and append-only
// write-back/reconciliation receipts. Connector transports remain in the SDK.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod write_back;
// MEDULLA-DURABLE-KERNEL-R084.1 — typed optimistic-write conflict and the closed
// set of scope-level merge strategies. Resolution and served wiring live above
// this pure-data crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod write_conflict;
