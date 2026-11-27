//! Private implementation module for the analytics job handler: the graph write of the
//! data-plane jobs (`DatasetProfile`, MEDULLA-UNIFIED-DATA-PLANE-R761.3, and
//! `QualityEvaluate`, MEDULLA-UNIFIED-DATA-PLANE-R762.3).
//!
//! Each job lowers what it computed into graph methods and commits them as ONE
//! engine-internal `MutationBatch` on the job's target graph, attributed to the job's
//! verified actor. The batch id is keyed by the job id, so a retried attempt replays the
//! committed batch instead of writing twice.

use super::prelude_server::*;
use super::*;
use serde_json::Value;

/// `AddNode` with `properties` encoded the way stored node properties are.
pub(super) fn node_method(id: String, properties: &Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: rmp_serde::to_vec_named(properties)
            .map_err(|_| "data plane node encoding failed".to_string())?,
    })
}

/// `AddEdge` with `properties` encoded the way stored edge properties are.
pub(super) fn edge_method(
    source: String,
    target: String,
    properties: &Value,
) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source,
        target_id: target,
        properties_msgpack: rmp_serde::to_vec_named(properties)
            .map_err(|_| "data plane edge encoding failed".to_string())?,
    })
}

/// Lower `upsert_node` / `upsert_edge` operations (the incident write shape of
/// `medulla_stream::telemetry::incident_nodes::incident_operations`) into graph methods.
pub(super) fn upsert_operations_to_methods(operations: &[Value]) -> Result<Vec<Method>, String> {
    let field = |operation: &Value, name: &str| -> Result<String, String> {
        operation
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("incident operation has no `{name}`"))
    };
    operations
        .iter()
        .map(|operation| {
            let properties = operation.get("properties").cloned().unwrap_or(Value::Null);
            match operation.get("op").and_then(Value::as_str) {
                Some("upsert_node") => node_method(field(operation, "id")?, &properties),
                Some("upsert_edge") => edge_method(
                    field(operation, "source")?,
                    field(operation, "target")?,
                    &properties,
                ),
                _ => Err("incident operation has an unknown op".to_string()),
            }
        })
        .collect()
}

/// Commit `methods` as one engine-internal `MutationBatch` on the job's target graph.
pub(super) async fn commit_job_methods(
    state: &Arc<RwLock<ServerState>>,
    job: &medulla_jobs::AnalyticsJob,
    namespace: &str,
    methods: Vec<Method>,
) -> Result<(), String> {
    if methods.is_empty() {
        return Ok(());
    }
    let (graph, _, core) = resolve_core_ref(state, &job.input_snapshot.graph)
        .await
        .ok_or_else(|| "analytics target graph is unavailable".to_string())?;
    let persistence = state.read().await.persistence.clone();
    let batch_id =
        medulla_server_api::batch_compile::opaque_coordinator_key(namespace, &graph, &job.job_id);
    let result = ResultPayload::Json(serde_json::json!({
        "job_id": job.job_id,
        "write": namespace,
    }));
    medulla_server_mutation::mutation_batch::commit_internal_graph_methods(
        medulla_server_mutation::mutation_batch::InternalGraphCommitRequest::new(
            persistence.as_ref(),
            &core,
            medulla_server_mutation::mutation_batch::CommitOrigin {
                request_id: 0,
                principal: Some(&job.policy.actor),
            },
            &graph,
            &batch_id,
            methods,
            &result,
        ),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn incident_operations_lower_to_node_and_edge_methods() {
        let operations = vec![
            serde_json::json!({"op": "upsert_node", "id": "inc-1", "properties": {"type": "Incident"}}),
            serde_json::json!({"op": "upsert_edge", "source": "inc-1", "target": "anom-1", "properties": {}}),
        ];
        let methods = upsert_operations_to_methods(&operations).unwrap();
        assert!(matches!(&methods[0], Method::AddNode { node_id, .. } if node_id == "inc-1"));
        assert!(matches!(&methods[1], Method::AddEdge { target_id, .. } if target_id == "anom-1"));
        assert!(upsert_operations_to_methods(&[serde_json::json!({"op": "drop"})]).is_err());
    }
}
