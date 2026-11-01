//! Per-domain graph-op handlers. The dispatch shell (super::dispatch_graph_op)
//! routes each method here; cross-cutting write side-effects (dirty/WAL/gauge)
//! stay centralized in the shell. Each module exposes `try_handle(...) ->
//! Result<Response, Method>`: `Ok` = handled, `Err(method)` = not mine, try next.

// M3 catalog-driven resharding admin (CONCEPT:MEDULLA-KG.backend.m3-admin-dispatch): the wire surface that DRIVES online
// resharding (MEDULLA-032), the tenant catalog (MEDULLA-031) and the rebalance planner (MEDULLA-035) +
// its execution (MEDULLA-039). Always declared; the real logic is `redb`-gated (the only build
// where the catalog/reshard/planner exist), and a non-redb build returns "not available".
pub(crate) mod admin;

// Raft cluster-membership ADMIN RPC (CONCEPT:MEDULLA-KG.storage.kg-kg-2 — cluster_deployment.md
// §5 item 2): exposes `MultiRaft::add_group_learner`/`change_group_voters`
// (raft/multi.rs) over `Method::RaftAddLearner`/`Method::RaftChangeMembership`. Always
// declared (like `placement`); the real answer is `raft`-gated, and a non-raft build
// answers a clean typed error.
pub(crate) mod raft_admin;
