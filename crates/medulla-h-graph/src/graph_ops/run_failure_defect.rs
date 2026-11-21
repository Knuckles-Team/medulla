//! A run that ends in failure files a defect (MEDULLA-SDLC-R003.3). When a served `AddNode` of
//! a `RunTrace` whose `terminationReason` is present and is not `final_answer` has committed,
//! the gateway files one symptom through the ingest seam (`IngestApi::file_defect`) and writes
//! the touched `Defect` node. The symptom text carries the reason and the task class, so equal
//! failures of one kind land on one defect; the source record is the run-trace id.

use medulla_protocol_types::protocol::{Method, Response};
use medulla_server_api::state::ServerState;
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_source_types::lifecycle::{
    defect_from_node, handle_file_defect, Defect, DefectIntake, FileDefectRequest, DEFECT_TYPE,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// The `terminationReason` of a run that ended well.
const FINAL_ANSWER: &str = "final_answer";
/// The most stored defects one filing reads.
const MAX_DEFECTS_READ: usize = 100_000;

/// The filing of a failed run, when `props` is a failed `RunTrace` node.
pub(super) fn failed_run_filing(
    node_id: &str,
    props: &Map<String, Value>,
    default_repo: &str,
    now_ms: u64,
) -> Option<FileDefectRequest> {
    if props.get("node_type").and_then(Value::as_str) != Some("RunTrace") {
        return None;
    }
    let reason = props.get("terminationReason").and_then(Value::as_str)?;
    if reason == FINAL_ANSWER {
        return None;
    }
    let text_of = |key: &str| props.get(key).and_then(Value::as_str).filter(|v| !v.trim().is_empty());
    let run = text_of("run_trace_id").unwrap_or(node_id);
    let task_class = text_of("runTaskClass").unwrap_or("unclassified");
    let observed_at = props
        .get("endedAtMs")
        .and_then(Value::as_u64)
        .unwrap_or(now_ms);
    let text = format!("run failed: {reason} (task class {task_class})");
    Some(FileDefectRequest {
        repo: text_of("repo").unwrap_or(default_repo).to_owned(),
        intake: DefectIntake::RunFailure,
        title: text.clone(),
        severity: None,
        source_record_id: run.to_owned(),
        observed_at,
        text,
    })
}

/// File the symptom of a failed run after its trace committed. `Ok(None)` when the written
/// method is not a failed run trace; `Ok(Some(response))` is the defect batch's response.
pub(super) async fn file_failed_run(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    method: &Method,
    now_ms: u64,
) -> Result<Option<Response>, String> {
    let Method::AddNode { node_id, properties_msgpack } = method else {
        return Ok(None);
    };
    let props = super::gateway_graph::decode_json_object(properties_msgpack);
    let Some(request) = failed_run_filing(node_id, &props, ctx.graph_name, now_ms) else {
        return Ok(None);
    };
    let request = handle_file_defect(request).map_err(|e| e.to_string())?;
    let seam = medulla_server_ops::api::ingest::ingest_seam(state)
        .await
        .ok_or_else(|| "INGEST_SEAM_UNAVAILABLE: the defect book is not installed".to_string())?;
    let mut existing: Vec<Defect> = ctx
        .core
        .get_nodes_by_label(DEFECT_TYPE, MAX_DEFECTS_READ)
        .into_iter()
        .filter_map(|(_, blob)| {
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            defect_from_node(&node, tenant_id)
        })
        .filter(|defect| defect.repo == request.repo)
        .collect();
    // The defect book numbers a new defect by position, so the stored order is the id order.
    existing.sort_by(|left, right| left.id.cmp(&right.id));
    let write = seam.file_defect(existing, request);
    let mut properties = write.node_properties()?;
    properties.insert("tenant_id".into(), tenant_id.into());
    if let Some(owner) = ctx.owner.or(ctx.caller) {
        properties.insert("_owner".into(), owner.into());
    }
    let operations = vec![json!({"op": "upsert_node", "id": write.node_id(), "properties": properties})];
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "defect batch encoding failed".to_string())?;
    let batch = Method::BatchUpdate { operations_msgpack };
    let plan = MutationPlan::for_method(&batch);
    // The defect is its own commit under a derived retry identity: the trace's nonce is spent.
    let idempotency_key = format!("{}:run-defect:{}", ctx.idempotency_key, write.node_id());
    let follow_on = MutationCtx {
        attempt_nonce: None,
        idempotency_key: &idempotency_key,
        ..*ctx
    };
    let none = std::collections::BTreeSet::new();
    Ok(super::gateway_graph::try_handle(state, tenant_id, &follow_on, &plan, &batch, &none).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(reason: Option<&str>) -> Map<String, Value> {
        let mut props = Map::new();
        props.insert("node_type".into(), "RunTrace".into());
        props.insert("runTaskClass".into(), "refactor".into());
        if let Some(reason) = reason {
            props.insert("terminationReason".into(), reason.into());
        }
        props
    }

    #[test]
    fn only_a_trace_that_ended_in_failure_is_filed() {
        assert!(failed_run_filing("t1", &trace(None), "g", 5).is_none());
        assert!(failed_run_filing("t1", &trace(Some("final_answer")), "g", 5).is_none());
        let mut other = trace(Some("error"));
        other.insert("node_type".into(), "Other".into());
        assert!(failed_run_filing("t1", &other, "g", 5).is_none());
        let filing = failed_run_filing("t1", &trace(Some("step_budget")), "g", 5).expect("filed");
        assert_eq!(filing.intake, DefectIntake::RunFailure);
        assert_eq!(filing.repo, "g");
        assert_eq!(filing.source_record_id, "t1");
        assert_eq!(filing.observed_at, 5);
        assert!(filing.text.contains("step_budget") && filing.text.contains("refactor"));
    }
}
