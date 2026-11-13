//! MEDULLA-DECISION-ENGINE-R038.2: a served `ToolActionAdmission` question
//! denies an action of a listed class that follows untrusted content, admits
//! one that follows trusted content, abstains when the content carries no
//! readable mark and refuses an unknown class. The content is read through the
//! caller's access check: a stranger and a foreign tenant are refused.

use super::*;
use medulla_decision_types::decision::{
    AbstainReason, EvidenceClass, PremiseClass, PremiseProvenance,
};

const GRAPH: &str = "content-graph";

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn action_request(schema: &ComponentDependency, content_ref: &str, class: &str) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::ToolActionAdmission,
        "admission",
        DecisionPolicyRef::Default,
        vec![
            text("graph", GRAPH),
            text("pending_call_id", "call-1"),
            text("params_digest", &"a".repeat(64)),
            text("trust_mark_ref", content_ref),
            text("action_class", class),
        ],
    )
}

fn verdict(batch: &DecisionBatch) -> String {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores[0].option_id.clone(),
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, .. } => format!("abstained:{}", code.as_str()),
            other => panic!("unexpected abstention {other:?}"),
        },
        other => panic!("unexpected outcome {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R038.2
#[tokio::test]
async fn untrusted_follow_up_is_denied() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        for (id, props) in [
            (
                "doc-untrusted",
                serde_json::json!({"trust_mark": "untrusted"}),
            ),
            ("doc-trusted", serde_json::json!({"trust_mark": "trusted"})),
            ("doc-unmarked", serde_json::json!({"summary": "no mark"})),
        ] {
            core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let (schema, _) = publish_route_schema(&h);

    // Each listed class that follows untrusted content is denied, with the
    // mark as an observed premise bound to the content node.
    for class in ["egress", "privileged_call", "delegation"] {
        let batch = decide(&h, action_request(&schema, "doc-untrusted", class))
            .await
            .unwrap();
        assert_eq!(verdict(&batch), "deny", "{class}");
        let record = &batch.records.as_slice()[0];
        assert_eq!(record.evidence_class, EvidenceClass::Observation);
        let premise = record
            .premises
            .iter()
            .find(|p| p.fact == "trust_mark:doc-untrusted")
            .expect("the mark is a named premise");
        assert_eq!(premise.class, PremiseClass::Observation);
        assert!(matches!(
            premise.provenance,
            PremiseProvenance::Observation { ref evaluation_id } if evaluation_id == "doc-untrusted"
        ));
    }

    // Following trusted content the action is admitted.
    let batch = decide(&h, action_request(&schema, "doc-trusted", "egress"))
        .await
        .unwrap();
    assert_eq!(verdict(&batch), "admit");

    // Content with no mark, and content the view does not hold, abstain.
    for missing in ["doc-unmarked", "doc-absent"] {
        let batch = decide(&h, action_request(&schema, missing, "egress"))
            .await
            .unwrap();
        assert_eq!(verdict(&batch), "abstained:TRUST_MARK_MISSING", "{missing}");
    }

    // An unknown class refuses as an invalid parameter naming its code.
    let error = decide(&h, action_request(&schema, "doc-trusted", "read"))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    assert!(error.contains("ACTION_CLASS_UNKNOWN"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = action_request(&schema, "doc-untrusted", "egress");
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");

    // A principal the graph's access check does not admit never sees the mark.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("stranger", TENANT);
    let denied = decode::<DecisionBatch>(
        super::statistical::handle_decide(
            &h.state,
            9,
            &stranger,
            action_request(&schema, "doc-untrusted", "egress"),
        )
        .await,
    );
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));
}
