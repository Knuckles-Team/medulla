//! Per-domain graph-op handlers. The dispatch shell (super::dispatch_graph_op)
//! routes each method here; cross-cutting write side-effects (dirty/WAL/gauge)
//! stay centralized in the shell. Each module exposes `try_handle(...) ->
//! Result<Response, Method>`: `Ok` = handled, `Err(method)` = not mine, try next.

// M3 catalog-driven resharding admin (CONCEPT:MEDULLA-KG.backend.m3-admin-dispatch): the wire surface that DRIVES online
// resharding (MEDULLA-032), the tenant catalog (MEDULLA-031) and the rebalance planner (MEDULLA-035) +
// its execution (MEDULLA-039). Always declared; the real logic is `redb`-gated (the only build
// where the catalog/reshard/planner exist), and a non-redb build returns "not available".
pub(crate) mod admin;
// Multi-op OCC ACID transactions (CONCEPT:MEDULLA-KG.txn.multi-op-occ-acid). Always present with the
// `server` feature — the Txn* methods are stateful (need `state`) and not
// feature-gated to a compute domain.
pub(crate) mod txn;
// Native time-series store + primitives (CONCEPT:AU-KG.retrieval.god-nodes-communities/211, feature `tsdb`). The
// Ts* methods are stateful (need the `SeriesStore` on `state`); a slim build without
// `tsdb` omits the module and the variants fall to the graph_ops not-built catch-all.
#[cfg(feature = "tsdb")]
pub(crate) mod timeseries;
// Distributed graph compute (CONCEPT:MEDULLA-KG.storage.feature, feature `compute-dist`). The
// DistributedCompute/*MatView methods drive the cross-shard Pregel engine + the
// matview store on ServerState; a non-cluster build omits the module and the variants
// fall to the graph_ops not-available catch-all.
// Present under EITHER `compute-dist` (the algo-only Pregel matview + DistributedCompute)
// OR `matview` (the plan-backed, single-node incremental matview) — the two families share
// this dispatch module but NOT a feature dependency (the plan-backed path uses no raft).
#[cfg(any(feature = "compute-dist", feature = "matview"))]
pub(crate) mod dist_compute;

// SQLite `.db` FILE import/export (CONCEPT:MEDULLA-KG.query.medulla-feature/MEDULLA-332, feature `sqlite-file`). The
// ImportSqliteFile/ExportSqliteFile methods move rows between an on-disk `sqlite3` `.db`
// file and the verified caller's owner-scoped `medulla_query::TableStore` (behind `query`), via the pure-Rust
// `medulla-sqlite-format` (no C sqlite). NOT graph-scoped; a build without `sqlite-file` omits
// the module and the variants aren't in the enum (so the dispatch chain never routes them).
#[cfg(feature = "sqlite-file")]
pub(crate) mod sqlite_file;
// Raft cluster-membership ADMIN RPC (CONCEPT:MEDULLA-KG.storage.kg-kg-2 — cluster_deployment.md
// §5 item 2): exposes `MultiRaft::add_group_learner`/`change_group_voters`
// (raft/multi.rs) over `Method::RaftAddLearner`/`Method::RaftChangeMembership`. Always
// declared (like `placement`); the real answer is `raft`-gated, and a non-raft build
// answers a clean typed error.
pub(crate) mod raft_admin;
