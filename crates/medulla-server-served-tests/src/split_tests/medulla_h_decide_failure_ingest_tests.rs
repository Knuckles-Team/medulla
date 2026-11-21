//! The served failure-pattern ingest: failed `RunTrace` nodes cluster into
//! patterns and a recurring pattern files exactly one gap node
//! (MEDULLA-DECISION-ENGINE-R701.3).

use medulla_decision_types::failure_patterns::{FailureIngestResult, FailureOp, Signature};
use medulla_h_decide::decide::stat_test_support::{decode, Harness, TENANT};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";

async fn dispatch(h: &Harness, method: Method) -> Response {
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 701,
                graph: GRAPH.into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method,
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes("decider", TENANT, &["*"]),
        ),
    )
    .await
}

/// One failed run of one signature: a plan step then a tool call that used
/// `search`, ended by `max_steps`.
fn failed_run(index: u64) -> Vec<Value> {
    let run = format!("run-{index}");
    let plan = format!("{run}-plan");
    let call = format!("{run}-call");
    vec![
        json!({"op": "upsert_node", "id": run, "properties": {
            "node_type": "RunTrace",
            "run_trace_id": format!("trace-{index}"),
            "terminationReason": "max_steps",
            "endMs": 1_700_000_000_000u64 + index,
        }}),
        json!({"op": "upsert_node", "id": plan, "properties": {"kind": "plan"}}),
        json!({"op": "upsert_node", "id": call, "properties": {"kind": "tool_call"}}),
        json!({"op": "upsert_edge", "source": run, "target": plan,
               "properties": {"relationship": "hasStep"}}),
        json!({"op": "upsert_edge", "source": run, "target": call,
               "properties": {"relationship": "hasStep"}}),
        json!({"op": "upsert_edge", "source": plan, "target": call,
               "properties": {"relationship": "precedes"}}),
        json!({"op": "upsert_edge", "source": call, "target": "search",
               "properties": {"relationship": "usesTool"}}),
    ]
}

async fn ingest(h: &Harness) -> FailureIngestResult {
    let response = dispatch(
        h,
        Method::FailureIngest {
            op: Box::new(FailureOp::Ingest {
                graph: GRAPH.into(),
                since_ms: None,
            }),
        },
    )
    .await;
    decode(response).unwrap()
}

/// Verifies MEDULLA-DECISION-ENGINE-R701.3: three failed runs of one
/// signature seeded through served `BatchUpdate` file one `gap:<id>` node, and
/// a second ingest files nothing.
// spec: MEDULLA-DECISION-ENGINE-R701.3
#[tokio::test]
async fn ingest_files_one_gap_per_topic() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let mut operations = vec![json!({"op": "upsert_node", "id": "search",
                                     "properties": {"kind": "tool"}})];
    for index in 0..3 {
        operations.extend(failed_run(index));
    }
    let seeded = dispatch(
        &h,
        Method::BatchUpdate {
            operations_msgpack: rmp_serde::to_vec_named(&operations).unwrap(),
        },
    )
    .await;
    assert!(seeded.refusal_text().is_none(), "{:?}", seeded.refusal_text());

    let first = ingest(&h).await;
    let signature = Signature("max_steps|search|tool_call".into());
    let topic_id = signature.topic_id();
    assert_eq!(first.filed, vec![topic_id.clone()]);
    let pattern = first
        .patterns
        .iter()
        .find(|pattern| pattern.signature == signature)
        .expect("the three runs cluster into one pattern");
    assert_eq!(pattern.count, 3);

    let second = ingest(&h).await;
    assert!(second.filed.is_empty(), "{:?}", second.filed);

    let guard = h.state.read().await;
    let core = &guard.registry.get(GRAPH).unwrap().core;
    let gaps = core.get_nodes_by_label("FailureGap", 100);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].0, format!("gap:{topic_id}"));
}
