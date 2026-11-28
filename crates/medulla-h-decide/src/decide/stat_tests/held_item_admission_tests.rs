//! MEDULLA-DECISION-ENGINE-R038.3: a served `HeldItemAdmission` question admits
//! a stored run program only when its digest matches, it is active, an
//! independent verifier checked it and the verification earned the policy's
//! `held_item.min_passes`; it admits or rejects an output proposal by the
//! critique verdict bound to its digest. Each refusal names its code, and the
//! item is read through the caller's access check: a stranger and a foreign
//! tenant are refused.

use super::*;
use medulla_decision_statistical_types::decision::knobs::KnobValue;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_types::decision::record::AbstainReason,
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
        StatisticalOutcome::Advisory { scores, .. } => scores.as_slice()[0].option_id.clone(),
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

fn typed(batch: &DecisionBatch) -> medulla_decision_types::decision::held_item::HeldItemAnswer {
    use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
    match &batch.records.as_slice()[0].answer {
        Some(FamilyAnswer::HeldItemAdmission(answer)) => (**answer).clone(),
        other => panic!("a typed held item answer, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R038.3
// spec: MEDULLA-DECISION-ENGINE-R038.5
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
    let typed_answer = typed(&batch);
    assert_eq!(
        typed_answer.outcome,
        medulla_decision_types::decision::held_item::HeldItemOutcome::Admitted
    );
    assert_eq!(typed_answer.verdict_ref, record.record_id);
    assert_eq!(typed_answer.verification_ref.as_deref(), Some("verif-1"));
    assert_eq!(typed_answer.verifier.as_deref(), Some("verifier"));
    assert_eq!(typed_answer.compiler.as_deref(), Some("compiler"));
    assert_eq!(typed_answer.reason, None);
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
        let mut policy =
            medulla_decision_statistical_types::decision::DecisionPolicy::engine_default();
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
    let rejected = decide(&h, proposal_request("prop-reject")).await.unwrap();
    assert_eq!(answer(&rejected), "rejected");
    let typed_rejection = typed(&rejected);
    assert_eq!(
        typed_rejection.outcome,
        medulla_decision_types::decision::held_item::HeldItemOutcome::Rejected
    );
    assert_eq!(typed_rejection.verification_ref.as_deref(), Some("crit-1"));
    let unbound = decide(&h, proposal_request("prop-unbound")).await.unwrap();
    let typed_refusal = typed(&unbound);
    assert_eq!(
        typed_refusal.outcome,
        medulla_decision_types::decision::held_item::HeldItemOutcome::Abstained
    );
    assert_eq!(typed_refusal.reason.as_deref(), Some("CRITIQUE_MISSING"));
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

// spec: MEDULLA-DECISION-ENGINE-R038.6
#[tokio::test]
async fn the_engine_records_the_critique_the_admission_then_reads() {
    use medulla_decision_types::decision::held_item::{
        critique_checklist, CritiqueAnswer, HeldItemCritiqueRecorded, HeldItemCritiqueRequest,
        HeldItemOutcome,
    };
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        guard.registry.get(GRAPH).unwrap().core.clone()
    };
    let bare = serde_json::json!({
        "held_item_kind": "run_output_proposal",
        "content_digest": digest('d'),
    });
    core.add_node(
        "prop-bare".to_string(),
        rmp_serde::to_vec_named(&bare).unwrap(),
    );
    core.add_node(
        "prog-1".to_string(),
        rmp_serde::to_vec_named(&program("compiler", Some("verifier"), 2, "active")).unwrap(),
    );
    let (schema, _) = publish_route_schema(&h);
    let ask = || {
        held_request(
            &schema,
            DecisionPolicyRef::Default,
            "run_output_proposal",
            "prop-bare",
            &digest('d'),
        )
    };

    // Without a critique the gate abstains and still names the questions to answer.
    let before = decide(&h, ask()).await.unwrap();
    assert_eq!(answer(&before), "abstained:CRITIQUE_MISSING");
    let questions = typed(&before).checklist;
    assert_eq!(questions, critique_checklist());
    assert_eq!(questions.len(), 4);

    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let critique = |passes: [bool; 4]| HeldItemCritiqueRequest {
        tenant_id: TENANT.to_string(),
        graph: GRAPH.to_string(),
        item_ref: "prop-bare".to_string(),
        content_digest: digest('d'),
        answers: questions
            .iter()
            .zip(passes)
            .map(|(question, passed)| CritiqueAnswer {
                id: question.id.clone(),
                passed,
            })
            .collect(),
    };
    let commit = |write: &super::super::planned_write::PlannedWrite| {
        for op in &write.operations {
            core.add_node(
                op["id"].as_str().unwrap().to_string(),
                rmp_serde::to_vec_named(&op["properties"]).unwrap(),
            );
        }
    };
    let recorded = |write: &super::super::planned_write::PlannedWrite| -> HeldItemCritiqueRecorded {
        match &write.reply {
            medulla_protocol_types::protocol::ResultPayload::Raw(bytes) => {
                rmp_serde::from_slice(bytes).unwrap()
            }
            other => panic!("unexpected payload {other:?}"),
        }
    };

    // An accepting critique is recorded on the node, and the gate now admits it.
    let plan = super::super::handle_held_item_critique(&h.state, &verified, critique([true; 4]))
        .await
        .unwrap();
    let accepted = recorded(&plan);
    assert_eq!(accepted.verdict, "accept");
    assert_eq!(plan.graph, GRAPH);
    commit(&plan);
    let after = decide(&h, ask()).await.unwrap();
    assert_eq!(answer(&after), "admitted");
    let admitted = typed(&after);
    assert_eq!(admitted.outcome, HeldItemOutcome::Admitted);
    assert_eq!(
        admitted.verification_ref.as_deref(),
        Some(accepted.critique_ref.as_str())
    );
    assert_eq!(
        admitted.critique_digest.as_deref(),
        Some(digest('d').as_str())
    );
    assert_eq!(admitted.critique_verdict.as_deref(), Some("accept"));
    let node =
        rmp_serde::from_slice::<serde_json::Value>(&core.get_node_properties("prop-bare").unwrap())
            .unwrap();
    assert_eq!(node["critique_recorded_by"], "decider");
    assert!(node["critique_recorded_at_ms"].as_u64().unwrap() > 0);
    assert_eq!(node["critique_answer_grounded"], true);
    assert_eq!(
        node["held_item_kind"], "run_output_proposal",
        "the node keeps its other fields"
    );

    // A failed question records a rejecting critique.
    let plan = super::super::handle_held_item_critique(
        &h.state,
        &verified,
        critique([true, false, true, true]),
    )
    .await
    .unwrap();
    assert_eq!(recorded(&plan).verdict, "reject");
    commit(&plan);
    assert_eq!(answer(&decide(&h, ask()).await.unwrap()), "rejected");

    // A stale digest, a program, an absent item, an incomplete critique and a foreign tenant
    // refuse and plan nothing.
    async fn refuse(
        h: &Harness,
        verified: &VerifiedRequestContext,
        request: HeldItemCritiqueRequest,
    ) -> String {
        super::super::handle_held_item_critique(&h.state, verified, request)
            .await
            .map(|_| ())
            .unwrap_err()
    }
    let mut stale = critique([true; 4]);
    stale.content_digest = digest('e');
    assert!(refuse(&h, &verified, stale)
        .await
        .starts_with("DIGEST_MISMATCH"));
    let mut program = critique([true; 4]);
    program.item_ref = "prog-1".to_string();
    assert!(refuse(&h, &verified, program)
        .await
        .starts_with("HELD_ITEM_UNKNOWN"));
    let mut absent = critique([true; 4]);
    absent.item_ref = "prop-absent".to_string();
    assert!(refuse(&h, &verified, absent)
        .await
        .starts_with("HELD_ITEM_UNKNOWN"));
    let mut incomplete = critique([true; 4]);
    incomplete.answers.pop();
    assert!(refuse(&h, &verified, incomplete)
        .await
        .starts_with("PARAMETER_INVALID"));
    let mut foreign = critique([true; 4]);
    foreign.tenant_id = "tenant-other".to_string();
    assert!(refuse(&h, &verified, foreign)
        .await
        .starts_with("ACCESS_DENIED"));
}
