//! Failure-pattern refusals and evidence (MEDULLA-DECISION-ENGINE-R701.4,
//! R701.5): the registered refusal codes, the `skipped` list, and a gap that
//! links back to every member run through `DERIVED_FROM` edges.

use super::super::failure_ingest::handle_failure_ingest;
use super::super::planned_write::PlannedWrite;
use super::*;
use medulla_decision_types::failure_patterns::{
    FailureIngestResult, FailureOp, GapTopic, Signature, FAILURE_NO_RUN_TRACE,
    PATTERN_BELOW_MIN_COUNT, TOPIC_NOT_FOUND, TOPIC_SOURCE_UNLINKED,
};
use medulla_protocol_types::protocol::ResultPayload;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";

fn verified(tenant: &str) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("decider", tenant, &["*"])
}

async fn core(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    h.state
        .read()
        .await
        .registry
        .get(GRAPH)
        .expect("graph exists")
        .core
        .clone()
}

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

fn link(core: &medulla_graph_core::graph::GraphCore, source: &str, target: &str, name: &str) {
    core.add_edge(
        source.to_string(),
        target.to_string(),
        rmp_serde::to_vec_named(&json!({"relationship": name})).unwrap(),
    )
    .unwrap();
}

/// One failed run of one signature: a plan step then a tool call that used
/// `search`, ended by `max_steps`. `trace` is its run-trace id, when it has one.
fn seed_run(core: &medulla_graph_core::graph::GraphCore, index: u64, trace: Option<&str>) {
    let run = format!("run-{index}");
    let plan = format!("{run}-plan");
    let call = format!("{run}-call");
    let mut props = json!({
        "node_type": "RunTrace",
        "terminationReason": "max_steps",
        "endMs": 1_700_000_000_000u64 + index,
    });
    if let Some(trace) = trace {
        props["run_trace_id"] = trace.into();
    }
    put(core, &run, props);
    put(core, &plan, json!({"kind": "plan"}));
    put(core, &call, json!({"kind": "tool_call"}));
    put(core, "search", json!({"kind": "tool"}));
    link(core, &run, &plan, "hasStep");
    link(core, &run, &call, "hasStep");
    link(core, &plan, &call, "precedes");
    link(core, &call, "search", "usesTool");
}

async fn plan(h: &Harness, tenant: &str, op: FailureOp) -> Result<PlannedWrite, String> {
    handle_failure_ingest(&h.state, &verified(tenant), op).await
}

fn refused(result: Result<PlannedWrite, String>) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("the operation must refuse"),
    }
}

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn ingest() -> FailureOp {
    FailureOp::Ingest {
        graph: GRAPH.into(),
        since_ms: None,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R701.4
#[tokio::test]
async fn failure_refusals_change_no_state() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = core(&h).await;

    // A pattern below the knob files no gap and is listed as skipped.
    seed_run(&core, 0, Some("trace-0"));
    let write = plan(&h, TENANT, ingest()).await.ok().expect("plans");
    assert!(write.operations.is_empty());
    let result: FailureIngestResult = reply(&write);
    assert!(result.filed.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].code, PATTERN_BELOW_MIN_COUNT.as_str());
    assert_eq!(result.skipped[0].count, 1);

    // An unknown topic is not found.
    let missing = FailureOp::Get {
        graph: GRAPH.into(),
        topic_id: "no-such-topic".into(),
    };
    let error = refused(plan(&h, TENANT, missing).await);
    assert!(error.starts_with(TOPIC_NOT_FOUND.as_str()), "{error}");

    // A source with no COVERS edge to the topic cannot mark it addressed.
    let signature = Signature("max_steps|search|tool_call".into());
    let topic_id = signature.topic_id();
    put(
        &core,
        &format!("gap:{topic_id}"),
        json!({"type": "FailureGap", "signature": signature.0, "status": "open",
               "tenant_id": TENANT}),
    );
    put(&core, "doc:1", json!({"kind": "document"}));
    let mark = FailureOp::MarkAddressed {
        graph: GRAPH.into(),
        topic_id: topic_id.clone(),
        by: "doc:1".into(),
    };
    let error = refused(plan(&h, TENANT, mark).await);
    assert!(error.starts_with(TOPIC_SOURCE_UNLINKED.as_str()), "{error}");
    link(&core, "doc:1", &format!("gap:{topic_id}"), "COVERS");
    let mark = FailureOp::MarkAddressed {
        graph: GRAPH.into(),
        topic_id,
        by: "doc:1".into(),
    };
    assert!(plan(&h, TENANT, mark).await.is_ok());

    // A failed run with no run-trace id refuses the ingest.
    seed_run(&core, 1, None);
    let error = refused(plan(&h, TENANT, ingest()).await);
    assert!(error.starts_with(FAILURE_NO_RUN_TRACE.as_str()), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R701.5
#[tokio::test]
async fn gap_links_back_to_its_member_runs() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = core(&h).await;
    for index in 0..3 {
        seed_run(&core, index, Some(&format!("trace-{index}")));
    }
    let write = plan(&h, TENANT, ingest()).await.ok().expect("plans");
    let result: FailureIngestResult = reply(&write);
    assert_eq!(result.filed.len(), 1);
    let topic_id = result.filed[0].clone();
    let gap = format!("gap:{topic_id}");

    // The gap node and one DERIVED_FROM edge per member are one batch.
    let edges: Vec<&Value> = write
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_edge")
        .collect();
    assert_eq!(edges.len(), 3);
    assert!(edges.iter().all(|edge| edge["source"] == gap
        && edge["properties"]["relationship"] == "DERIVED_FROM"));
    assert_eq!(write.operations[0]["op"], "upsert_node");
    let properties = &write.operations[0]["properties"];
    assert_eq!(properties["count"], 3);
    assert!(properties.get("first_seen").is_some());
    assert!(properties.get("last_seen").is_some());
    assert!(properties.get("minimal_steps").is_some());

    // Commit the batch the way the router does, then read the gap back.
    for op in &write.operations {
        match op["op"].as_str().unwrap() {
            "upsert_node" => put(&core, op["id"].as_str().unwrap(), op["properties"].clone()),
            _ => link(
                &core,
                op["source"].as_str().unwrap(),
                op["target"].as_str().unwrap(),
                "DERIVED_FROM",
            ),
        }
    }
    let get = FailureOp::Get {
        graph: GRAPH.into(),
        topic_id: topic_id.clone(),
    };
    let read = plan(&h, TENANT, get).await.ok().expect("reads");
    assert!(read.operations.is_empty());
    let topic: GapTopic = reply(&read);
    assert_eq!(topic.members, ["run-0", "run-1", "run-2"]);

    // Another tenant's read finds no such topic.
    let foreign = FailureOp::Get {
        graph: GRAPH.into(),
        topic_id,
    };
    let error = refused(plan(&h, "tenant-other", foreign).await);
    assert!(error.starts_with(TOPIC_NOT_FOUND.as_str()), "{error}");
}
