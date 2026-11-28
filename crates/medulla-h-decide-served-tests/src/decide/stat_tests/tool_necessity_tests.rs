//! Tool necessity served through `Decide` (MEDULLA-DECISION-ENGINE-R165.3):
//! `RunTrace` nodes the caller can see are measured, the model's call is
//! compared with the measured necessity, and too few runs abstain.

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_h_decide_stat_tests::decide::stat_tests::kernel_family_support::kernel_request;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_server_api::authority_context::VerifiedRequestContext;
use {
    medulla_h_decide_stat_tests::decide::stat_tests::path_preference_tests::pins,
    medulla_h_decide_stat_tests::decide::stat_tests::path_preference_tests::OTHER,
};
use {
    medulla_identity_core_types::acl::AgentIdentity, medulla_identity_core_types::acl::AgentRole,
};

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
    trace_rows_at(start, count, used, succeeded, now)
}

/// [`trace_rows`] whose runs started at `now`.
fn trace_rows_at(
    start: usize,
    count: usize,
    used: bool,
    succeeded: bool,
    now: u64,
) -> Vec<(String, serde_json::Value)> {
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

/// Publish the tool and the model profile the necessity question names.
fn publish_known(h: &Harness, tool: bool, model: bool) {
    if tool {
        h.publish_test_component(TOOL, AgentComponentKind::Tool, "search", None::<&()>, None)
            .unwrap();
    }
    if model {
        h.publish_model_profile(TENANT, MODEL, 4096, None, None)
            .unwrap();
    }
}

async fn seed(h: &Harness, graph: &str, rows: Vec<(String, serde_json::Value)>) {
    seed_owned(
        h,
        graph,
        medulla_protocol_types::protocol::GraphType::Team,
        None,
        rows,
    )
    .await;
}

async fn seed_owned(
    h: &Harness,
    graph: &str,
    graph_type: medulla_protocol_types::protocol::GraphType,
    owner: Option<&str>,
    rows: Vec<(String, serde_json::Value)>,
) {
    let core = {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(graph, graph_type, owner.map(str::to_owned))
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
    publish_known(&h, true, true);
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

// spec: MEDULLA-DECISION-ENGINE-R165.5
#[tokio::test]
async fn mismatch_links_to_the_model_call() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    publish_known(&h, true, true);
    let mut rows = trace_rows(0, 30, true, true);
    rows.extend(trace_rows(30, 6, false, true));
    rows.extend(trace_rows(36, 24, false, false));
    seed(&h, "kg-premises", rows).await;

    let skipped = decide(&h, necessity_request(&pins.0, "kg-premises", false))
        .await
        .unwrap();
    let premises = &skipped.records.as_slice()[0].premises;
    for subject in [
        "tool_necessity.score",
        "tool_necessity.samples_with",
        "tool_necessity.samples_without",
        "tool_necessity.provenance_digest",
        "tool_necessity.comparison",
    ] {
        assert!(
            premises.iter().any(|p| p.subject == subject),
            "{subject} missing from {premises:?}"
        );
    }
    let mismatch = premises
        .iter()
        .find(|p| p.subject == "tool_necessity.mismatch:missed_tool")
        .expect("the mismatch premise");
    assert_eq!(mismatch.fact, "call:1");
    assert!(matches!(
        &mismatch.provenance,
        medulla_decision_statistical_types::decision::record::premise::PremiseProvenance::Request { field }
            if field == "model_call_id"
    ));

    // A model that agrees records no mismatch premise.
    let called = decide(&h, necessity_request(&pins.0, "kg-premises", true))
        .await
        .unwrap();
    assert!(!called.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.subject.starts_with("tool_necessity.mismatch")));
}

// spec: MEDULLA-DECISION-ENGINE-R165.3
#[tokio::test]
async fn too_few_runs_abstain() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    publish_known(&h, true, true);
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

/// A caller of another tenant, who is no owner of the graph and holds no grant on it, reads
/// none of its run traces: the question is refused before any trace is measured.
// spec: MEDULLA-DECISION-ENGINE-R165.3
#[tokio::test]
async fn other_tenant_traces_are_not_read() {
    let mut isolation = ServerState::test_isolation("decider");
    isolation.register_agent(AgentIdentity {
        agent_id: "outsider".to_owned(),
        role: AgentRole::Agent,
        teams: Vec::new(),
        roles: Vec::new(),
    });
    let h = Harness::with_isolation(isolation).await;
    let pins = pins(&h, TENANT);
    let theirs = pins_of_other(&h);
    publish_known(&h, true, true);
    let mut rows = trace_rows(0, 30, true, true);
    rows.extend(trace_rows(30, 6, false, true));
    rows.extend(trace_rows(36, 24, false, false));
    seed_owned(
        &h,
        "kg-private-traces",
        medulla_protocol_types::protocol::GraphType::Agent,
        Some("decider"),
        rows,
    )
    .await;

    // The owner measures them.
    let own = decide(&h, necessity_request(&pins.0, "kg-private-traces", false))
        .await
        .unwrap();
    assert_eq!(chosen(&own.records.as_slice()[0])[1], "agreement:false");

    // The other tenant's principal is refused.
    let refused = decode::<DecisionBatch>(
        medulla_h_decide::decide::statistical::handle_decide(
            &h.state,
            1,
            &VerifiedRequestContext::verified_for_test_in_tenant("outsider", OTHER),
            necessity_request(&theirs, "kg-private-traces", false),
        )
        .await,
    )
    .unwrap_err();
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}

/// The pin of the other tenant's schema, published for it.
fn pins_of_other(h: &Harness) -> ComponentDependency {
    pins(h, OTHER).0
}

/// A tool or model the tenant never published refuses, and a measurement whose newest run is
/// older than `tool_necessity.max_age` abstains instead of being estimated forward.
// spec: MEDULLA-DECISION-ENGINE-R165.4
#[tokio::test]
async fn unknown_measurements_refuse() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    // Runs from three days ago: inside the 30 day window, past the one day max_age.
    let three_days = now.saturating_sub(3 * 86_400_000);
    let mut rows = trace_rows_at(0, 30, true, true, three_days);
    rows.extend(trace_rows_at(30, 30, false, false, three_days));
    seed(&h, "kg-old-traces", rows).await;

    let refusal_of = |outcome: Result<DecisionBatch, String>| outcome.unwrap_err();

    let none_published =
        refusal_of(decide(&h, necessity_request(&pins.0, "kg-old-traces", true)).await);
    assert!(none_published.contains("TOOL_UNKNOWN"), "{none_published}");
    assert!(
        none_published.starts_with("PARAMETER_INVALID"),
        "{none_published}"
    );

    publish_known(&h, true, false);
    let no_model = refusal_of(decide(&h, necessity_request(&pins.0, "kg-old-traces", true)).await);
    assert!(no_model.contains("MODEL_UNKNOWN"), "{no_model}");
    assert!(no_model.starts_with("PARAMETER_INVALID"), "{no_model}");

    publish_known(&h, false, true);
    let stale = decide(&h, necessity_request(&pins.0, "kg-old-traces", true))
        .await
        .unwrap();
    assert!(
        format!("{:?}", stale.records.as_slice()[0].outcome).contains("STALE_MEASUREMENT"),
        "{:?}",
        stale.records.as_slice()[0].outcome
    );
}
