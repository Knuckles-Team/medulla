//! MEDULLA-DECISION-ENGINE-R137.15: a served `SubCallAdmission` question
//! admits a call only when its stored signature, input schema, tool set and
//! model all match, and abstains with the expected and actual value otherwise.

use std::time::Instant;

use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalDecisionRecord, StatisticalOutcome, TypedParam,
    TypedValue,
};
use medulla_decision_types::decision::sub_call::{
    tool_set_digest, SubCallRequest, SubCallSignature, SUB_CALL_SIGNATURE_MARKER,
};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicyRef, EvidenceClass};

use super::super::stat_executor::ExecutionContext;
use super::*;

fn stored() -> SubCallSignature {
    SubCallSignature {
        signature_id: "sig".to_string(),
        revision: 2,
        digest: "digest-2".to_string(),
        input_schema_digest: "in-schema".to_string(),
        output_schema_digest: "out-schema".to_string(),
        admitted_tool_ids: vec!["search".to_string(), "read".to_string()],
        model_id: "model-a".to_string(),
    }
}

fn call() -> SubCallRequest {
    SubCallRequest {
        parent_run_id: "run-1".to_string(),
        parent_span_id: "span-1".to_string(),
        call_id: "call-1".to_string(),
        signature_id: "sig".to_string(),
        signature_revision: 2,
        signature_digest: "digest-2".to_string(),
        input_digest: "input-1".to_string(),
        input_schema_digest: "in-schema".to_string(),
        tool_ids: vec!["read".to_string()],
        model_id: "model-a".to_string(),
    }
}

fn view(
    signature: Option<&SubCallSignature>,
    parent: bool,
) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let put = |id: String, props: serde_json::Value| {
        core.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
    };
    if let Some(signature) = signature {
        let mut props = serde_json::to_value(signature).unwrap();
        props[SUB_CALL_SIGNATURE_MARKER] = serde_json::json!(true);
        put(SubCallSignature::node_id(&signature.signature_id), props);
    }
    if parent {
        put(
            "run:run-1".to_string(),
            serde_json::json!({ "kind": "run" }),
        );
    }
    core.analysis_snapshot()
}

fn request(schema: &ComponentDependency, call: &SubCallRequest) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::SubCallAdmission,
        "admit",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text("signatures".to_string()),
            },
            TypedParam {
                name: "sub_call".to_string(),
                value: TypedValue::Text(serde_json::to_string(call).unwrap()),
            },
        ],
    )
}

fn run(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> StatisticalDecisionRecord {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
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
    .unwrap()
    .records
    .as_slice()[0]
        .clone()
}

/// The abstention code and detail, or `None` for an admission.
fn abstention(record: &StatisticalDecisionRecord) -> Option<(String, String)> {
    match &record.outcome {
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, detail, .. } => {
                Some((code.as_str().to_string(), detail.clone()))
            }
            other => panic!("unexpected abstention {other:?}"),
        },
        StatisticalOutcome::Advisory { .. } => None,
        other => panic!("unexpected outcome {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R137.15
#[tokio::test]
async fn admits_a_matching_call() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let signature = stored();
    let record = run(
        &h,
        &request(&schema, &call()),
        &view(Some(&signature), true),
    );
    assert_eq!(abstention(&record), None);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    // The record binds the signature, model, tool set and schema digests.
    for fact in [
        "sub_call.signature_digest=digest-2".to_string(),
        "sub_call.model_id=model-a".to_string(),
        format!(
            "sub_call.tool_set_digest={}",
            tool_set_digest(&call().tool_ids)
        ),
        "sub_call.input_schema_digest=in-schema".to_string(),
    ] {
        assert!(
            record.premises.iter().any(|p| p.fact == fact),
            "premise {fact}"
        );
    }
    // The recorded result must carry the signature's output schema digest.
    assert!(signature.result_matches("out-schema"));
    assert!(!signature.result_matches("other"));
}

// spec: MEDULLA-DECISION-ENGINE-R137.15
#[tokio::test]
async fn each_mismatch_refuses_with_expected_and_actual() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let signature = stored();
    let refuse = |call: SubCallRequest, view: medulla_graph_core::graph::GraphView| {
        abstention(&run(&h, &request(&schema, &call), &view)).expect("abstains")
    };
    let ok_view = || view(Some(&signature), true);

    let (code, _) = refuse(call(), view(None, true));
    assert_eq!(code, "SUBCALL_SIGNATURE_UNKNOWN");

    let (code, detail) = refuse(
        SubCallRequest {
            signature_digest: "digest-1".to_string(),
            ..call()
        },
        ok_view(),
    );
    assert_eq!(code, "SUBCALL_SIGNATURE_CHANGED");
    assert!(
        detail.contains("digest-2") && detail.contains("digest-1"),
        "{detail}"
    );

    let (code, detail) = refuse(
        SubCallRequest {
            signature_revision: 1,
            ..call()
        },
        ok_view(),
    );
    assert_eq!(code, "SUBCALL_SIGNATURE_CHANGED");
    assert!(detail.contains('2') && detail.contains('1'), "{detail}");

    let (code, detail) = refuse(
        SubCallRequest {
            model_id: "model-b".to_string(),
            ..call()
        },
        ok_view(),
    );
    assert_eq!(code, "SUBCALL_SIGNATURE_CHANGED");
    assert!(
        detail.contains("model-a") && detail.contains("model-b"),
        "{detail}"
    );

    let (code, detail) = refuse(
        SubCallRequest {
            input_schema_digest: "other-schema".to_string(),
            ..call()
        },
        ok_view(),
    );
    assert_eq!(code, "SUBCALL_INPUT_SCHEMA_MISMATCH");
    assert!(
        detail.contains("in-schema") && detail.contains("other-schema"),
        "{detail}"
    );

    let rogue = SubCallRequest {
        tool_ids: vec!["read".to_string(), "delete".to_string()],
        ..call()
    };
    let (code, detail) = refuse(rogue.clone(), ok_view());
    assert_eq!(code, "SUBCALL_TOOL_NOT_ADMITTED");
    assert!(detail.contains("delete"), "{detail}");
    assert!(
        detail.contains(&tool_set_digest(&signature.admitted_tool_ids)),
        "{detail}"
    );
    assert!(
        detail.contains(&tool_set_digest(&rogue.tool_ids)),
        "{detail}"
    );

    let (code, _) = refuse(call(), view(Some(&signature), false));
    assert_eq!(code, "SUBCALL_PARENT_MISSING");
    let (code, _) = refuse(
        SubCallRequest {
            parent_span_id: String::new(),
            ..call()
        },
        ok_view(),
    );
    assert_eq!(code, "SUBCALL_PARENT_MISSING");
}
