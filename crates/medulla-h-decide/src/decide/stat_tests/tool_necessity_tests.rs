//! Tool necessity served through `Decide` (MEDULLA-DECISION-ENGINE-R165.3):
//! `RunTrace` nodes the caller can see are measured, the model's call is
//! compared with the measured necessity, and too few runs abstain.

use super::kernel_family_support::kernel_request;
use super::path_preference_tests::pins;
use super::*;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};

const TOOL: &str = "tool:search";
const MODEL: &str = "model:a";

/// `count` run-trace nodes of `model`: each used the tool or not, each succeeded or not.
fn trace_rows(
    start: usize,
    count: usize,
    used: bool,
    succeeded: bool,
) -> Vec<(String, serde_json::Value)> {
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    (start..start + count)
        .map(|index| {
            let steps = if used {
                serde_json::json!([{"index": 0, "kind": "tool_call", "action_ref": TOOL, "outcome": "ok"}])
            } else {
                serde_json::json!([])
            };
            (
                format!("trace-{index}"),
                serde_json::json!({
                    "kind": "run_trace",
                    "run_trace": {
                        "schema_version": 1,
                        "run_id": format!("run-{index}"),
                        "task_signature": "task",
                        "model_id": MODEL,
                        "steps": steps,
                        "outcome": if succeeded { "succeeded" } else { "failed" },
                        "started_at_ms": now,
                    },
                }),
            )
        })
        .collect()
}

async fn seed(h: &Harness, graph: &str, rows: Vec<(String, serde_json::Value)>) {
    let core = {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                graph,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        guard.registry.get(graph).unwrap().core.clone()
    };
    for (id, props) in rows {
        core.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
        core.mark_dirty();
    }
}

fn necessity_request(
    schema: &ComponentDependency,
    graph: &str,
    model_called: bool,
) -> DecideRequest {
    let param = |name: &str, value: TypedValue| TypedParam {
        name: name.to_owned(),
        value,
    };
    let mut request = kernel_request(
        schema,
        QuestionKind::ToolNecessity,
        "tool_id",
        TypedValue::Iri(TOOL.to_owned()),
        &["tool"],
    );
    request.params = BoundedVec::new(vec![
        param("graph", TypedValue::Text(graph.to_owned())),
        param("model_call_id", TypedValue::Text("call:1".to_owned())),
        param("model_called", TypedValue::Bool(model_called)),
        param("model_id", TypedValue::Iri(MODEL.to_owned())),
        param("tool_id", TypedValue::Iri(TOOL.to_owned())),
    ])
    .unwrap();
    request
}

fn chosen(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .iter()
            .map(|scored| scored.option_id.clone())
            .collect(),
        other => panic!("an advisory answer, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R165.3
#[tokio::test]
async fn necessity_journey() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    // With the tool 30 of 30 runs succeed; without it 6 of 30: the tool is necessary.
    let mut rows = trace_rows(0, 30, true, true);
    rows.extend(trace_rows(30, 6, false, true));
    rows.extend(trace_rows(36, 24, false, false));
    seed(&h, "kg-traces", rows).await;

    // A model that skipped the necessary tool disagrees: a missed tool.
    let skipped = decide(&h, necessity_request(&pins.0, "kg-traces", false))
        .await
        .unwrap();
    let options = chosen(&skipped.records.as_slice()[0]);
    assert!(options[0].starts_with("necessity:"), "{options:?}");
    assert_eq!(options[1], "agreement:false");
    assert_eq!(options[2], "mismatch:missed_tool");

    // A model that called it agrees and records no mismatch.
    let called = decide(&h, necessity_request(&pins.0, "kg-traces", true))
        .await
        .unwrap();
    let options = chosen(&called.records.as_slice()[0]);
    assert_eq!(options.len(), 2, "{options:?}");
    assert_eq!(options[1], "agreement:true");
}

// spec: MEDULLA-DECISION-ENGINE-R165.3
#[tokio::test]
async fn too_few_runs_abstain() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    let mut rows = trace_rows(0, 3, true, true);
    rows.extend(trace_rows(3, 3, false, false));
    seed(&h, "kg-thin", rows).await;
    let batch = decide(&h, necessity_request(&pins.0, "kg-thin", true))
        .await
        .unwrap();
    assert!(
        format!("{:?}", batch.records.as_slice()[0].outcome).contains("INSUFFICIENT_SAMPLES"),
        "{:?}",
        batch.records.as_slice()[0].outcome
    );
}
