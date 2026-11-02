//! medulla-work-types: Work items, work market, control leases, host start, jobs, worker registry, run history and orchestration record types.
//!
//! Level L1a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R700 — governed action specs and runs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod action_spec;
// MEDULLA-UNIFIED-DATA-PLANE-R750.750-752 — run audit rows, tool calls and the
// scoped conversation-history step store.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_history;
// MEDULLA-DURABLE-KERNEL-R074.1 — pure worker registry, registration validation,
// and deterministic aggregate capabilities. No RPC or persistence ownership.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod worker_registry;
// MEDULLA-DECISION-ENGINE-R617.1 — the engine-held memory records: the conversation
// record a chat log parses into, the layered project rules, the memory
// retention policy and the context budget. Pure data; parsing, pruning and
// the covering-set optimizer are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod memory;
// CONCEPT:INT-P2-1 — the durable analytics-job plane's wire op (`JobOp`), gated
// `jobs`. Lives here (not in `medulla-jobs`, which sits ABOVE medulla-core in the DAG) for the
// SAME reason `acl::RbacAdminOp` does: `protocol::Method::AnalyticsJob` carries it
// over the wire, and `protocol` is bottom-of-DAG. Pure serde — no dep.
#[cfg(feature = "jobs")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod jobs;
// MEDULLA-ORCH-VOCAB-R801/R802 — the versioned harness-component vocabulary and the
// improvement library beside it: typed model + pure rules. The durable write
// (ChangeEnvelope/AddNode) and the served reads live upstream.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod orchestration;
// AU wire-first native control-plane operations: bounded capacity leases and
// WorkItem admission/submission.  This module is hand-written until the AU
// protocol catalog generator can express the bounded maps and native enums.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod native_control;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_checkpoint;
// GOC-19 — the WorkItem submission command-log admission core (tenant-scoped
// idempotency replay + per-authority fencing), built on GOC-03's
// `commit_descriptor::CommitDescriptor` currency. Unconditional, pure
// data/logic; the native protocol/storage adapter now lives in
// `native_control`, `server::mutation_batch`, and `redb_store`. See
// `plans/graph-os-completion-program/lanes/GOC-19-atomic-workitem-command-log.md`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_command_log;
// EH-219 — typed, tenant-bound WorkItem reads (`GetWorkItem`/`ListWorkItems`):
// the caller's row view, its three-bound page scan, and its cursor family.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_completion;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_read;
// graph-os MEDULLA-2 — native control leases: a tenant-bound, time-boxed grant with
// a one-way active -> revoked|expired lifecycle (not a capacity admission).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod control_lease;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod host_start;
// EH-348 — the graph-driven work market: the canonical `:Gap` paired with its
// one native WorkItem, the versioned derived `WorkOffer`, and the deterministic
// legal-offer ranking stage the `Decide` layer runs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_market;
// D-VZ-1 (lanes V4 "engine integration" / V6 "graph-native marks") — the native
// visualization engine's wire op (`VizOp`), gated `viz`. Lives here (not in
// `medulla-viz-core`, which sits in a separate small leaf DAG, not below medulla-types) for
// the SAME reason `jobs`/`statechart` do: `protocol::Method::Viz` carries it over
// the wire, and `protocol` is bottom-of-DAG. Pure serde — no dep on
// medulla-viz-core/medulla-viz-columnstore/medulla-viz-export; the handler
// (`src/server/handlers/viz.rs`, facade feature `viz-static-export`) is the one
// place that parses `VizRenderRequest::spec_json` into a real
// `medulla_viz_core::ViewSpec` and resolves it.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_usage;
// MEDULLA-FEDERATED-QUERY-R702 — search-task records and shortcut-risk reports.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod search_task;
