//! medulla-exec: Physical plan execution: optimizer, cost models, runtime, DAG, incremental and streamed results.
//!
//! Level L5 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (execution).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod algebra;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cost;
// MEDULLA-UNIFIED-DATA-PLANE-R006 — typed pushdown decision/fallback-reason,
// refusing a self-contradictory pushed-down-with-fallback state.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pushdown_plan;
/// Execution budgets (UQL-09): typed resource bounds on result size and traversal fan-out.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod budget;
/// The typed DAG plan representation (CONCEPT:MEDULLA-KG.query.plan-dag, E5 phase 1): [`dag::PlanDag`]
/// generalizes the linear [`Plan`] into a real graph of operators (a node's `inputs` name
/// its dependency nodes), with a lossless conversion from every existing linear `Plan` (a
/// degenerate chain). `Op` is unchanged; execution over a `PlanDag` lives in
/// [`dag_exec`]. Sits beside `exec`/`knowledge` under the same `query` gate (a `PlanNode`
/// carries an optional `dag::ProjectionSchema`).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dag;
/// Physical execution of a [`dag::PlanDag`] (CONCEPT:MEDULLA-KG.query.plan-dag-exec, E5 phase 2) —
/// [`dag_exec::execute_dag`] runs ALONGSIDE the untouched [`exec::execute`]; for a
/// degenerate linear-chain dag it is byte-identical to it (the differential oracle proves
/// this over the whole fixture suite).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dag_exec;
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod exec;
/// DBSP-style INCREMENTAL materialized-view maintenance (CONCEPT:MEDULLA-KG.storage.incremental-matview):
/// a dep-free Z-set delta model + a circuit compiler for the LINEAR operator subset
/// (`Scan`/`Filter`/`AsOf`/`WindowAgg`(linear)/`Limit`), generalizing `src/server/cdc.rs`'s
/// hand-rolled `ContinuousQuery`/`maintain()` mini-DBSP to an arbitrary supported `Plan`.
/// An unsupported op returns [`incremental::UnsupportedOp`] so the server keeps that view
/// on the recompute path — a per-view fallback, never a silently-wrong incremental result.
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod incremental;
/// Per-row, SQL-three-valued evaluation of the relational predicate algebra (UQL-01) —
/// the Rust twin of the FILTER leg's SQL for incremental maintenance and edge predicates.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pred_eval;
/// The physical EXECUTION runtime (CONCEPT:MEDULLA-KG.query.parallel-runtime) — Lane B. The
/// `execute_ops` driver-dispatch `execute` routes through + the rayon-morsel, memory-accounted,
/// spilling `ParallelDriver` (feature `par-runtime`). Gated on `query` (it schedules the
/// `query`-tier physical fns); a default/Pi build dispatches to the Lane-0 `SerialDriver`.
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod runtime;
/// The cross-modal cost-based optimizer (CONCEPT:MEDULLA-KG.query.xmodal-cost-optimizer) — Lane A's
/// rule engine over the logical `Vec<Op>` that [`exec::plan_optimize`] calls to reorder
/// operators across modalities into a cheaper-but-equivalent plan. Compiled under `query`;
/// runtime kill-switch `MEDULLA_COST_OPT=0` → identity passthrough.
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod optimizer;
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod oracle;
/// SQL text crossing a federation boundary: dialects, identifiers, literals (one owner).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sql_text;
/// KAN-learned cardinality-estimate correction (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization,
/// W4.12 phase 2): an online, per-op-kind correction trained continuously from the
/// SAME `(estimated, actual)` pairs [`exec::run_with_adaptive_reopt`] already observes,
/// so a systematic estimation bias learned on one query nudges later queries' plan-time
/// estimates instead of only triggering reactive re-planning. Opt-in twice over — the
/// `learned-cost` cargo feature AND the `MEDULLA_LEARNED_COST=1` runtime flag —
/// so a default build/deployment is byte-for-byte unaffected. See the module doc for
/// the design and the "queryable artifact" surface.
#[cfg(feature = "learned-cost")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod learned_cost;
/// Read-time access checks for `checked_at_read` sources (MEDULLA-FEDERATED-QUERY-R703).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod access_check;
/// Column mask and row-filter policies (MEDULLA-FEDERATED-QUERY-R706).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod policy_mask;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rowset;
/// Stored, versioned queries (MEDULLA-FEDERATED-QUERY-R708).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stored_query;
#[cfg(all(feature = "query", any(test, feature = "test-support")))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod fixture;
