//! MEDULLA-DECISION-ENGINE-R137.13: a served `RecursionMode` question selects
//! direct or recursive handling from measured signals, and refuses bad signals
//! with a registered code under `PARAMETER_INVALID`.

use std::time::Instant;

use medulla_decision_outcome_types::decision::context_budget::ContextBudgetSignals;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, DecisionPolicyRef, EvidenceClass};

use super::super::stat_executor::ExecutionContext;
use super::*;

const NOW_MS: u64 = 1_000_000;
const TOOL_CALL: &str = "tool-call:t1";
const SNAPSHOT: &str = "graph-snapshot:g1";

fn signals(bytes: u64, nodes: u64, observed_at_ms: u64) -> ContextBudgetSignals {
    let mut signals = ContextBudgetSignals {
        run_id: "run-1".to_string(),
        tool_output_bytes: bytes,
        tool_output_digest: "sha256:out".to_string(),
        tool_call_ref: TOOL_CALL.to_string(),
        graph_snapshot_ref: SNAPSHOT.to_string(),
        graph_node_count: nodes,
        observed_at_ms,
        signal_digest: String::new(),
    };
    signals.signal_digest = signals.compute_digest();
    signals
}

fn view(nodes: &[&str]) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    for id in nodes {
        core.add_node(
            id.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({ "kind": "signal-ref" })).unwrap(),
        );
    }
    core.analysis_snapshot()
}

fn request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    signals_json: String,
) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::RecursionMode,
        "mode",
        policy,
        vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text("signals-graph".to_string()),
            },
            TypedParam {
                name: "signals".to_string(),
                value: TypedValue::Text(signals_json),
            },
        ],
    )
}

fn run(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<medulla_decision_types::decision::statistical::StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: NOW_MS,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        Some(view),
        "decider".to_string(),
        Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn answer(
    record: &medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            scores.iter().map(|s| s.option_id.clone()).collect()
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn decide_signals(
    h: &Harness,
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    signals: &ContextBudgetSignals,
) -> Result<Vec<String>, String> {
    let json = serde_json::to_string(signals).unwrap();
    run(
        h,
        &request(schema, policy, json),
        &view(&[TOOL_CALL, SNAPSHOT]),
    )
    .map(|r| answer(&r))
}

// spec: MEDULLA-DECISION-ENGINE-R137.13
#[tokio::test]
async fn thresholds_select_mode() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let default = || DecisionPolicyRef::Default;
    let at = |bytes, nodes| decide_signals(&h, &schema, default(), &signals(bytes, nodes, NOW_MS));

    // Default knobs: 65536 bytes, 50000 nodes.
    assert_eq!(at(65_535, 49_999).unwrap(), ["mode:direct", "BELOW_BOTH"]);
    assert_eq!(
        at(65_536, 49_999).unwrap(),
        ["mode:recursive", "OUTPUT_BYTES_AT_OR_ABOVE"]
    );
    assert_eq!(
        at(65_535, 50_000).unwrap(),
        ["mode:recursive", "GRAPH_NODES_AT_OR_ABOVE"]
    );
    assert_eq!(
        at(70_000, 60_000).unwrap(),
        ["mode:recursive", "OUTPUT_BYTES_AT_OR_ABOVE"]
    );

    // The pinned knobs move both thresholds.
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("context.recursive_output_bytes"),
        KnobValue::Integer(2_000),
    );
    policy.knobs.insert(
        KnobName::new("context.recursive_graph_nodes"),
        KnobValue::Integer(200),
    );
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-recursion", &policy),
    };
    let moved = decide_signals(&h, &schema, pinned, &signals(1_999, 199, NOW_MS)).unwrap();
    assert_eq!(moved, ["mode:direct", "BELOW_BOTH"]);
    let moved = decide_signals(
        &h,
        &schema,
        DecisionPolicyRef::Pinned {
            component: h.publish_policy("policy-recursion-2", &policy),
        },
        &signals(1_999, 200, NOW_MS),
    )
    .unwrap();
    assert_eq!(moved, ["mode:recursive", "GRAPH_NODES_AT_OR_ABOVE"]);

    // The record carries the measures and the inputs digest as observations.
    let sealed = signals(70_000, 10, NOW_MS);
    let json = serde_json::to_string(&sealed).unwrap();
    let record = run(
        &h,
        &request(&schema, DecisionPolicyRef::Default, json),
        &view(&[TOOL_CALL, SNAPSHOT]),
    )
    .unwrap();
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    for fact in [
        "recursion_mode.tool_output_bytes=70000".to_string(),
        "recursion_mode.graph_node_count=10".to_string(),
        format!("recursion_mode.inputs_digest={}", sealed.signal_digest),
    ] {
        assert!(
            record.premises.iter().any(|p| p.fact == fact),
            "premise {fact}"
        );
    }
}

// spec: MEDULLA-DECISION-ENGINE-R137.13
#[tokio::test]
async fn bad_signals_refuse() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let refused = |json: String, view: &medulla_graph_core::graph::GraphView| {
        run(
            &h,
            &request(&schema, DecisionPolicyRef::Default, json),
            view,
        )
        .unwrap_err()
    };
    let good_view = view(&[TOOL_CALL, SNAPSHOT]);
    let good = signals(100, 100, NOW_MS);
    let text = |s: &ContextBudgetSignals| serde_json::to_string(s).unwrap();

    // A tampered measure no longer matches the digest.
    let mut tampered = good.clone();
    tampered.tool_output_bytes = 1;
    let error = refused(text(&tampered), &good_view);
    assert!(error.contains("PARAMETER_INVALID"), "{error}");
    assert!(error.contains("SIGNAL_DIGEST_MISMATCH"), "{error}");

    // Older than the 60 second default.
    let stale = signals(100, 100, NOW_MS - 60_001);
    let error = refused(text(&stale), &good_view);
    assert!(error.contains("PARAMETER_INVALID"), "{error}");
    assert!(error.contains("SIGNAL_STALE"), "{error}");
    // Exactly at the limit is fresh.
    assert!(decide_signals(
        &h,
        &schema,
        DecisionPolicyRef::Default,
        &signals(1, 1, NOW_MS - 60_000)
    )
    .is_ok());

    // A reference that does not resolve in the caller's tenant graph.
    for missing in [TOOL_CALL, SNAPSHOT] {
        let others: Vec<&str> = [TOOL_CALL, SNAPSHOT]
            .into_iter()
            .filter(|id| *id != missing)
            .collect();
        let error = refused(text(&good), &view(&others));
        assert!(error.contains("PARAMETER_INVALID"), "{error}");
        assert!(error.contains("SIGNAL_CROSS_TENANT"), "{error}");
        assert!(error.contains(missing), "{error}");
    }

    // A negative, missing or non-numeric value is invalid; so is an empty ref.
    let mut value = serde_json::to_value(&good).unwrap();
    value["tool_output_bytes"] = serde_json::json!(-1);
    let negative = value.to_string();
    let mut value = serde_json::to_value(&good).unwrap();
    value.as_object_mut().unwrap().remove("graph_node_count");
    let missing = value.to_string();
    let mut value = serde_json::to_value(&good).unwrap();
    value["observed_at_ms"] = serde_json::json!("soon");
    let text_number = value.to_string();
    let mut empty_ref = good.clone();
    empty_ref.tool_call_ref.clear();
    for json in [
        negative,
        missing,
        text_number,
        text(&empty_ref),
        "[1]".to_string(),
    ] {
        let error = refused(json, &good_view);
        assert!(error.contains("PARAMETER_INVALID"), "{error}");
        assert!(error.contains("SIGNAL_INVALID"), "{error}");
    }
}
