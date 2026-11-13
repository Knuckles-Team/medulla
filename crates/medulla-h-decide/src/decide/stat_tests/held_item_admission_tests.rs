//! MEDULLA-DECISION-ENGINE-R038.3: a served `HeldItemAdmission` question admits
//! a stored run program only when its digest matches, it is active, an
//! independent verifier checked it and the verification earned the policy's
//! `held_item.min_passes`; it admits or rejects an output proposal by the
//! critique verdict bound to its digest. Each refusal names its code, and the
//! item is read through the caller's access check: a stranger and a foreign
//! tenant are refused.

use super::*;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::{
    AbstainReason, EvidenceClass, PremiseClass, PremiseProvenance,
};

const GRAPH: &str = "held-items";

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn digest(c: char) -> String {
    c.to_string().repeat(64)
}

fn held_request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    kind: &str,
    item_ref: &str,
    expected: &str,
) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::HeldItemAdmission,
        "admission",
        policy,
        vec![
            text("graph", GRAPH),
            text("item_kind", kind),
            text("item_ref", item_ref),
            text("content_digest", expected),
        ],
    )
}

fn answer(batch: &DecisionBatch) -> String {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores[0].option_id.clone(),
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, .. } => format!("abstained:{}", code.as_str()),
            other => panic!("unexpected abstention {other:?}"),
        },
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn program(
    compiled_by: &str,
    verified_by: Option<&str>,
    passes: i64,
    lifecycle: &str,
) -> serde_json::Value {
    let mut row = serde_json::json!({
        "held_item_kind": "stored_run_program",
        "content_digest": digest('a'),
        "lifecycle": lifecycle,
        "compiled_by": compiled_by,
        "verification_passes": passes,
    });
    if let Some(verifier) = verified_by {
        row["verified_by"] = verifier.into();
        row["verification_ref"] = "verif-1".into();
    }
    row
}

fn proposal(verdict: &str, critique_digest: &str) -> serde_json::Value {
    serde_json::json!({
        "held_item_kind": "run_output_proposal",
        "content_digest": digest('b'),
        "critique_ref": "crit-1",
        "critique_digest": critique_digest,
        "critique_verdict": verdict,
    })
}

// spec: MEDULLA-DECISION-ENGINE-R038.3
#[tokio::test]
async fn program_needs_independent_verifier_and_passes() {
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
                "prog-ok",
                program("compiler", Some("verifier"), 2, "active"),
            ),
            ("prog-self", program("same", Some("same"), 5, "active")),
            ("prog-unverified", program("compiler", None, 0, "active")),
            (
                "prog-few",
                program("compiler", Some("verifier"), 1, "active"),
            ),
            (
                "prog-retired",
                program("compiler", Some("verifier"), 9, "retired"),
            ),
            ("prop-accept", proposal("accept", &digest('b'))),
            ("prop-reject", proposal("reject", &digest('b'))),
            ("prop-unbound", proposal("accept", &digest('c'))),
        ] {
            core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let (schema, _) = publish_route_schema(&h);
    let default = || DecisionPolicyRef::Default;
    let program_request = |id: &str, expected: &str| {
        held_request(&schema, default(), "stored_run_program", id, expected)
    };
    let proposal_request =
        |id: &str| held_request(&schema, default(), "run_output_proposal", id, &digest('b'));

    // A verified, active program with enough passes is admitted; the
    // verification is a named observed premise bound to the item.
    let batch = decide(&h, program_request("prog-ok", &digest('a')))
        .await
        .unwrap();
    assert_eq!(answer(&batch), "admitted");
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    let premise = record
        .premises
        .iter()
        .find(|p| p.fact == "verification:verif-1")
        .expect("the verification is a named premise");
    assert_eq!(premise.class, PremiseClass::Observation);
    assert!(matches!(
        premise.provenance,
        PremiseProvenance::Observation { ref evaluation_id } if evaluation_id == "prog-ok"
    ));

    // Every program refusal names its code.
    for (id, expected, code) in [
        ("prog-ok", digest('f'), "DIGEST_MISMATCH"),
        ("prog-self", digest('a'), "VERIFIER_NOT_INDEPENDENT"),
        ("prog-unverified", digest('a'), "VERIFIER_NOT_INDEPENDENT"),
        ("prog-few", digest('a'), "PASSES_INSUFFICIENT"),
        ("prog-retired", digest('a'), "RETIRED"),
        ("prog-absent", digest('a'), "HELD_ITEM_UNKNOWN"),
        ("prop-accept", digest('b'), "HELD_ITEM_UNKNOWN"),
    ] {
        let batch = decide(&h, program_request(id, &expected)).await.unwrap();
        assert_eq!(answer(&batch), format!("abstained:{code}"), "{id}");
    }

    // The pass floor is the policy's knob: raised to 3 a two-pass program is
    // refused, lowered to 1 a one-pass program is admitted.
    let knob = &medulla_decision_types::decision::held_item::KNOBS[0];
    let pinned = |floor: i64| {
        let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
        policy
            .knobs
            .insert(knob.knob_name(), KnobValue::Integer(floor));
        DecisionPolicyRef::Pinned {
            component: h.publish_policy(&format!("policy-held-{floor}"), &policy),
        }
    };
    let strict = held_request(
        &schema,
        pinned(3),
        "stored_run_program",
        "prog-ok",
        &digest('a'),
    );
    assert_eq!(
        answer(&decide(&h, strict).await.unwrap()),
        "abstained:PASSES_INSUFFICIENT"
    );
    let lax = held_request(
        &schema,
        pinned(1),
        "stored_run_program",
        "prog-few",
        &digest('a'),
    );
    assert_eq!(answer(&decide(&h, lax).await.unwrap()), "admitted");

    // A proposal follows the critique verdict bound to its digest.
    let batch = decide(&h, proposal_request("prop-accept")).await.unwrap();
    assert_eq!(answer(&batch), "admitted");
    assert!(batch.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.fact == "critique:crit-1"));
    assert_eq!(
        answer(&decide(&h, proposal_request("prop-reject")).await.unwrap()),
        "rejected"
    );
    assert_eq!(
        answer(&decide(&h, proposal_request("prop-unbound")).await.unwrap()),
        "abstained:CRITIQUE_MISSING"
    );

    // A malformed digest and an unknown kind refuse as invalid parameters.
    let error = decide(&h, program_request("prog-ok", "ABC"))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    let error = decide(
        &h,
        held_request(&schema, default(), "program", "prog-ok", &digest('a')),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = program_request("prog-ok", &digest('a'));
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");

    // A principal the graph's access check does not admit never sees the item.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("stranger", TENANT);
    let denied = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(
            &h.state,
            9,
            &stranger,
            program_request("prog-ok", &digest('a')),
        )
        .await,
    );
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));
}
