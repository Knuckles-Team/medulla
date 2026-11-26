//! medulla-work-types: Work items, work market, control leases, host start, jobs, worker registry, run history and orchestration record types.
//!
//! Layer L1a (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-SCM-R008 — the verdict of one verification gate.
pub mod step_verdict;
// MEDULLA-UNIFIED-DATA-PLANE-R700 — governed action specs and runs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod action_spec;
// MEDULLA-DURABLE-KERNEL-R095.1000 — served agent-bus participant roster model.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_bus;
// MEDULLA-ORCH-VOCAB-R009.18.1 — the measured-or-unknown run observation window.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_observation;
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
// MEDULLA-DECISION-ENGINE-R617.7 — the participant record of a session.
pub mod participant;
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
// MEDULLA-DURABLE-KERNEL-R750.750.1 — the signed agent-turn envelope and its
// dispatch carrier.
pub mod agent_turn;
// GOC-19 — the WorkItem submission command-log admission core (tenant-scoped
// idempotency replay + per-authority fencing), built on GOC-03's
// `commit_descriptor::CommitDescriptor` currency. Unconditional, pure
// data/logic; the native protocol/storage adapter now lives in
// `native_control`, `server::mutation_batch`, and `redb_store`. See
// `plans/graph-os-completion-program/lanes/GOC-19-atomic-workitem-command-log.md`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_command_log;
// MEDULLA-UNIFIED-DATA-PLANE-R766 — the closed registry of admitted work-item kinds.
pub mod work_item_kind;
// EH-219 — typed, tenant-bound WorkItem reads (`GetWorkItem`/`ListWorkItems`):
// the caller's row view, its three-bound page scan, and its cursor family.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod run_completion;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_read;
// MEDULLA-ORCH-VOCAB-R812 — the WorkItem lifecycle as one explicit state machine:
// the edge table W01-W26 and `WorkItemStatus::apply`, the only transition function.
pub mod work_item_machine;
// graph-os MEDULLA-2 — native control leases: a tenant-bound, time-boxed grant with
// a one-way active -> revoked|expired lifecycle (not a capacity admission).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod control_lease;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod host_start;
#[allow(missing_docs)]
pub mod source_query_approval;
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
/// The reply of the served `DowntimeSelections` method (MEDULLA-DURABLE-KERNEL-R101.3).
pub mod downtime;
// MEDULLA-DURABLE-KERNEL-R076.1 — authoritative scheduler policy data and validation.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod scheduler_config;
// MEDULLA-DECISION-ENGINE-R173.7 — the harness edit lifecycle.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod harness_edit;
// MEDULLA-DECISION-ENGINE-R816 — run-bound observation state.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod observation_state;
// MEDULLA-FEDERATED-QUERY-R702 — search-task records and shortcut-risk reports.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod search_task;
#[cfg(feature = "statechart")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod statechart;
// MEDULLA-DURABLE-KERNEL-R071 — typed durable task-queue model with refusals.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod task_queue;
