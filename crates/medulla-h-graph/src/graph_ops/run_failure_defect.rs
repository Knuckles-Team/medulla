//! A run that ends in failure files a defect (MEDULLA-SDLC-R003.3). A served `AddNode` of
//! a `RunTrace` whose `terminationReason` is present and is not `final_answer` has committed,
//! the gateway plans one symptom through the ingest seam (`IngestApi::file_defect`) BEFORE the
//! commit and writes the touched `Defect` node in the same batch as the trace's `add_node`, so
//! a committed trace is never reported as a failure and never lacks its defect. The symptom text carries the reason and the task class, so equal
//! failures of one kind land on one defect; the source record is the run-trace id.

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_server_mutation::mutation::MutationCtx;
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
    let text_of = |key: &str| {
        props
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
    };
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

/// The one batch that commits a failed run trace together with its defect: the trace's own
/// `add_node` first, then the defect's `upsert_node`. A pure fold, so the same bytes are
/// tested and committed.
pub(super) fn fold_trace_and_defect(
    trace_id: &str,
    trace_properties: &Map<String, Value>,
    defect_id: &str,
    defect_properties: Map<String, Value>,
) -> Result<Method, String> {
    let operations = vec![
        json!({"op": "add_node", "id": trace_id, "properties": trace_properties}),
        json!({"op": "upsert_node", "id": defect_id, "properties": defect_properties}),
    ];
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "run trace batch encoding failed".to_string())?;
    Ok(Method::BatchUpdate { operations_msgpack })
}

/// Plan the filing of a failed run before its trace commits (plan-then-apply): `Ok(None)` when
/// the method is not a failed run trace; `Ok(Some(batch))` is the batch to commit INSTEAD of
/// the bare `AddNode`, carrying the trace and its defect, so they commit together or not at
/// all. An `Err` refuses the whole write and nothing commits.
pub(super) async fn plan_failed_run(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    method: &Method,
    now_ms: u64,
) -> Result<Option<Method>, String> {
    let Method::AddNode {
        node_id,
        properties_msgpack,
    } = method
    else {
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
    fold_trace_and_defect(node_id, &props, &write.node_id(), properties).map(Some)
}

/// A committed folded batch answers as the `AddNode` the caller sent.
pub(super) fn answer_as_add_node(mut response: Response) -> Response {
    if response.refusal_text().is_none() {
        response.result = Some(ResultPayload::scalar::<
            medulla_protocol_types::result_contract::graph::AddNode,
        >("ok".to_string()));
    }
    response
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

    fn ops(method: &Method) -> Vec<Value> {
        let Method::BatchUpdate { operations_msgpack } = method else {
            panic!("a failed run folds into one BatchUpdate");
        };
        rmp_serde::from_slice(operations_msgpack).expect("decodes")
    }

    // MEDULLA-SDLC-R003.3: the trace and its defect commit in one batch, trace first.
    #[test]
    fn the_trace_and_its_defect_commit_in_one_batch() {
        let mut defect = Map::new();
        defect.insert("type".into(), "Defect".into());
        let batch = fold_trace_and_defect("t1", &trace(Some("error")), "defect:g:1", defect)
            .expect("folds");
        let operations = ops(&batch);
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0]["op"], "add_node");
        assert_eq!(operations[0]["id"], "t1");
        assert_eq!(operations[0]["properties"]["terminationReason"], "error");
        assert_eq!(operations[1]["op"], "upsert_node");
        assert_eq!(operations[1]["id"], "defect:g:1");
    }

    // MEDULLA-SDLC-R003.3: a defect that cannot be planned refuses before any commit, so no
    // trace is written without it (the refusal is the planning step's `Err`, never a post-commit one).
    #[test]
    fn a_defect_that_cannot_be_planned_refuses_the_write_before_any_commit() {
        let mut props = trace(Some("error"));
        props.insert("repo".into(), "  ".into());
        let filing = failed_run_filing("t1", &props, "", 5).expect("a failed trace");
        assert!(filing.repo.trim().is_empty());
        assert!(handle_file_defect(filing).is_err());
    }

    #[test]
    fn a_committed_folded_batch_answers_as_the_add_node_the_caller_sent() {
        let answered = answer_as_add_node(Response::ok(1, ResultPayload::Bool(true)));
        assert!(answered.refusal_text().is_none());
        let refused = answer_as_add_node(Response::err(2, "REFUSED: no".to_string()));
        assert!(refused.refusal_text().is_some());
    }
}
