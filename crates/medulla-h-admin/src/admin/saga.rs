//! Cluster-admin saga helpers (catalog sagas and rebalance reports) over the shared
//! admin saga ledger.

#[cfg(feature = "redb")]
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_persistence::AsRedb;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_server_ops::api::admin::saga::begin_admin_saga_with_nonce;
use medulla_server_ops::api::admin::saga::finish_admin_saga;
use medulla_server_ops::api::admin::saga::AdminSaga;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Nonce;

#[cfg(feature = "redb")]
fn catalog_saga_replay_response(
    req_id: u64,
    method: &Method,
    saga: &AdminSaga,
) -> Option<Response> {
    if saga.prepared {
        return Some(Response::err(
            req_id,
            "catalog saga is Prepared; refusing to re-execute its mutation",
        ));
    }
    let result = saga.replayed.clone()?;
    if let Err(error) = saga
        .batch
        .result_contract
        .expect("method-bearing catalog saga has a result contract")
        .validate(&result)
    {
        return Some(Response::err(
            req_id,
            format!(
                "catalog saga replay for {} violates its durable result contract: {error}",
                method.tag_name(),
            ),
        ));
    }
    Some(Response::ok(req_id, result))
}

/// Run one catalog mutation under an admin saga.
#[cfg(feature = "redb")]
pub(crate) fn catalog_saga<M>(
    req_id: u64,
    caller: Option<&str>,
    backend: &medulla_persistence::redb_backend::RedbBackend,
    method: &Method,
    attempt_nonce: Option<Nonce>,
    apply: impl FnOnce(&medulla_persistence::tenant_catalog::TenantCatalog) -> Result<(), String>,
) -> Response
where
    M: medulla_protocol_types::result_contract::MethodResult<
        Body = bool,
        Encoding = medulla_protocol_types::result_contract::encoding::Bool,
    >,
{
    let saga = match begin_admin_saga_with_nonce(
        backend,
        req_id,
        caller,
        method,
        DurabilityDomain::ControlPlane,
        attempt_nonce,
    ) {
        Ok(saga) => saga,
        Err(error) => return Response::err(req_id, error),
    };
    if let Some(response) = catalog_saga_replay_response(req_id, method, &saga) {
        return response;
    }
    let Some(catalog) = backend.catalog() else {
        return no_catalog(req_id);
    };
    if let Err(error) = apply(&catalog) {
        return Response::err(req_id, format!("catalog write failed: {error}"));
    }
    match finish_admin_saga(
        backend,
        saga.batch,
        saga.created_at_ms,
        medulla_protocol_types::protocol::ResultPayload::scalar::<M>(true),
    ) {
        Ok(result) => Response::ok(req_id, result),
        Err(error) => Response::err(req_id, error),
    }
}

/// Build the reshard result payload from a report.
#[cfg(feature = "redb")]
pub(crate) fn reshard_report(
    report: &medulla_persistence::online_reshard::ReshardReport,
) -> medulla_protocol_types::result_contract::cluster::ShardReshardReport {
    medulla_protocol_types::result_contract::cluster::ShardReshardReport {
        graph: report.graph.clone(),
        from_shard: report.from_shard as u64,
        to_shard: report.to_shard as u64,
        nodes: report.nodes,
        edges: report.edges,
        ledger: report.ledger,
        semantic: report.semantic,
        audit: report.audit,
        delta_nodes: report.delta_nodes,
        delta_edges: report.delta_edges,
        no_op: report.no_op,
    }
}

/// Build the rebalance plan result payload.
#[cfg(feature = "redb")]
pub(crate) fn rebalance_plan_report(
    plan: &medulla_persistence::rebalance::RebalancePlan,
    shards: &[medulla_persistence::rebalance::ShardLoad],
) -> medulla_protocol_types::result_contract::cluster::RebalancePlanReport {
    medulla_protocol_types::result_contract::cluster::RebalancePlanReport {
        moves: plan
            .moves
            .iter()
            .map(
                |planned| medulla_protocol_types::result_contract::cluster::RebalanceMove {
                    graph: planned.graph.clone(),
                    from_shard: planned.from_shard,
                    to_shard: planned.to_shard,
                },
            )
            .collect(),
        shards: shards
            .iter()
            .map(
                |shard| medulla_protocol_types::result_contract::cluster::ShardLoadSummary {
                    shard: shard.shard,
                    total: shard.total(),
                    graphs: shard.graphs.len() as u64,
                },
            )
            .collect(),
    }
}

/// The refusal for a build with no tenant catalog.
#[cfg(feature = "redb")]
pub(crate) fn no_catalog(req_id: u64) -> Response {
    Response::err(
        req_id,
        "no tenant catalog attached (set MEDULLA_TENANT_CATALOG=1 and restart)",
    )
}

/// Rebalance options from the request parameters.
#[cfg(feature = "redb")]
pub(crate) fn rebalance_opts(
    tolerance: Option<f64>,
    max_moves: Option<usize>,
) -> medulla_persistence::rebalance::RebalanceOptions {
    let mut opts = medulla_persistence::rebalance::RebalanceOptions::default();
    if let Some(t) = tolerance {
        opts.tolerance = t;
    }
    if let Some(m) = max_moves {
        opts.max_moves = m;
    }
    opts
}

/// Live per-graph load `(sanitized_fname, resident_node_count)` over the registry + the
/// shard count K (CONCEPT:MEDULLA-KG.sharding.even-load-rebalance integration). Resident node count is the KG-2.51 per-graph
/// size dimension — a cheap, available balance metric. `__commons__` is included like any
/// other graph. Returns `(loads, k)`.
#[cfg(feature = "redb")]
pub(crate) async fn live_graph_loads(
    state: &Arc<RwLock<ServerState>>,
) -> (Vec<(String, u64)>, usize) {
    let s = state.read().await;
    let loads: Vec<(String, u64)> = s
        .registry
        .all_entries()
        .iter()
        .map(|e| {
            (
                medulla_kernel::persist::sanitize(&e.name),
                e.core.node_count() as u64,
            )
        })
        .collect();
    let k = s
        .persistence
        .as_ref()
        .and_then(|p| p.as_redb())
        .map(|r| r.shard_count())
        .unwrap_or(1);
    (loads, k)
}
