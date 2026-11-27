//! The served connector run methods (MEDULLA-UNIFIED-DATA-PLANE-R790): `ConnectorRuns` and
//! `ConnectorRunRecord`.
//!
//! Each arm reads only the nodes the caller can see in the request graph, decides in the pure
//! functions of `medulla_source_types::connector_run` and, for the one write, commits the
//! `ConnectorRun` node in one gateway-routed batch. A refusal and a replay return before the batch
//! is built, so they write nothing. The start and the end of a run are the engine clock.

use super::deploy::{verified_tenant, visible_nodes_of};
use super::*;

use medulla_protocol_types::result_contract::ingestion::{
    ConnectorRunGet as ConnectorRunGetResult, ConnectorRunRecord as ConnectorRunRecordResult,
    ConnectorRuns as ConnectorRunsResult,
};
use medulla_source_types::connector_run::{
    connector_run_node_id, handle_connector_run_get, handle_connector_run_record,
    handle_connector_runs, ConnectorRun, ConnectorRunInput, RunState,
    CONNECTOR_RUN_COUNTS_PROPERTY, CONNECTOR_RUN_TYPE,
};
use medulla_types::transition_refused::MachineState;

/// Most run nodes one list reads.
const MAX_RUNS_READ: usize = 100_000;

/// `ConnectorRuns`: one page of the tenant's runs the caller can see.
pub(super) async fn list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    connection_id: Option<String>,
    run_state: Option<RunState>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let runs: Vec<ConnectorRun> =
            visible_nodes_of(state, &target, CONNECTOR_RUN_TYPE, MAX_RUNS_READ)
                .await?
                .iter()
                .filter_map(|(_, node)| ConnectorRun::from_node(node, tenant))
                .collect();
        handle_connector_runs(
            runs,
            tenant,
            connection_id.as_deref(),
            run_state,
            limit,
            cursor.as_deref(),
        )
        .map_err(|e| e.to_string())
    }
    .await;
    typed_response::<ConnectorRunsResult>(target.req_id, result)
}

/// `ConnectorRunRecord`: decide against the stored run of the tenant, then write the one node.
pub(super) async fn record(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    input: ConnectorRunInput,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        input.validate().map_err(|e| e.to_string())?;
        let node_id = connector_run_node_id(tenant, &input.run_id);
        let stored = visible_node(state, &target, &node_id)
            .await?
            .and_then(|node| ConnectorRun::from_node(&node, tenant));
        let write = handle_connector_run_record(
            stored.as_ref(),
            input,
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        )
        .map_err(|e| e.to_string())?;
        if !write.replayed {
            let run = &write.run;
            let counts = serde_json::to_string(&run.counts)
                .map_err(|_| "connector run encoding failed".to_string())?;
            let mut properties = Map::new();
            properties.insert("type".into(), CONNECTOR_RUN_TYPE.into());
            properties.insert("run_id".into(), run.run_id.clone().into());
            properties.insert("connection_id".into(), run.connection_id.clone().into());
            properties.insert("state".into(), run.state.stored().into());
            properties.insert("started_ms".into(), run.started_ms.into());
            if let Some(finished_ms) = run.finished_ms {
                properties.insert("finished_ms".into(), finished_ms.into());
            }
            properties.insert(CONNECTOR_RUN_COUNTS_PROPERTY.into(), counts.into());
            stamp(&mut properties, &target);
            let operations = bounded(vec![
                json!({"op": "upsert_node", "id": node_id, "properties": properties}),
            ])?;
            commit(state, &target, operations).await?;
        }
        Ok(write.receipt(node_id))
    }
    .await;
    typed_response::<ConnectorRunRecordResult>(target.req_id, result)
}

/// `ConnectorRunGet`: the stored run of one id. An absent, foreign, invisible or malformed run id
/// refuses alike.
pub(super) async fn get(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    run_id: String,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let stored = visible_node(state, &target, &connector_run_node_id(tenant, &run_id))
            .await?
            .and_then(|node| ConnectorRun::from_node(&node, tenant));
        handle_connector_run_get(stored, &run_id).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<ConnectorRunGetResult>(target.req_id, result)
}
