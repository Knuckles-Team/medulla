//! The `playbook_triage` family through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R704.3): a burst of inbound events selects the
//! tenant's playbooks by trigger, the answer is an advisory list of proposed
//! steps in playbook then step order, a write step is named with its governed
//! route and never run, and one tenant never sees another tenant's playbooks.
//! A request to execute is refused, a burst no playbook answers abstains
//! naming the trigger kind, and a proposal records the playbook id and
//! revision (MEDULLA-DECISION-ENGINE-R704.4, R704.5).

use super::*;
use medulla_agent_types::agent_ontology::CAPABILITY_ROOT;
use medulla_decision_types::decision::playbook::PARAM_EXECUTE;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::{
    AbstainReason, PremiseClass, PremiseProvenance, ResolutionKind,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

const OTHER_TENANT: &str = "tenant-other";
const STEP_SCORE: i64 = 1 << 32;

fn publish_playbook(h: &Harness, tenant: &str, id: &str, trigger: &str, class: Option<&str>) {
    let steps = |ids: &[(&str, bool)]| {
        let items: Vec<String> = ids
            .iter()
            .map(|(step, writes)| {
                format!(r#"{{"step_id":"{step}","action_ref":"action:{step}","writes":{writes}}}"#)
            })
            .collect();
        format!("[{}]", items.join(","))
    };
    let body = match id {
        "playbook:flap" => steps(&[("s1", false), ("s2", true)]),
        "playbook:down" => steps(&[("d1", false)]),
        "playbook:ticket" => steps(&[("t1", false)]),
        "playbook:pressure" => steps(&[("p1", false)]),
        _ => steps(&[("o1", false)]),
    };
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Playbook;
    draft.attributes.insert("trigger".into(), trigger.into());
    draft.attributes.insert("steps".into(), body);
    if let Some(class) = class {
        draft.attributes.insert("ticket_class".into(), class.into());
    }
    h.commit_in(tenant, draft, None).unwrap();
}

fn event(id: &str, service: &str, kind: &str, classes: &[&str], at_ms: i64) -> DeclaredOption {
    let mut classification = vec![service.to_string(), kind.to_string()];
    classification.extend(classes.iter().map(|c| (*c).to_string()));
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(classification).unwrap(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "at_ms".to_string(),
            q32: at_ms,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

fn triage_request(schema: &ComponentDependency, tenant: &str) -> DecideRequest {
    let mut request = declared_request(
        schema,
        tenant,
        QuestionKind::PlaybookTriage,
        "unused",
        DecisionPolicyRef::Default,
        Vec::new(),
    );
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![
            event("e1", "svc-a", "state_change", &[], 0),
            event("e2", "svc-a", "state_change", &[], 1_000),
            event("e3", "svc-a", "state_change", &[], 2_000),
            event("e4", "svc-b", "service_down", &[], 2_500),
            event("e5", "svc-c", "ticket", &["eg:capability/analysis"], 3_000),
        ])
        .unwrap(),
    };
    request
}

fn options(batch: &DecisionBatch) -> Vec<(String, i64)> {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .as_slice()
            .iter()
            .map(|s| (s.option_id.clone(), s.score.value))
            .collect(),
        other => panic!("expected an advisory answer, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R704.3
#[tokio::test]
async fn decide_routes_playbook_triage() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_playbook(&h, TENANT, "playbook:flap", "service_flapping", None);
    publish_playbook(&h, TENANT, "playbook:down", "service_down", None);
    publish_playbook(&h, TENANT, "playbook:pressure", "resource_pressure", None);
    publish_playbook(
        &h,
        TENANT,
        "playbook:ticket",
        "ticket_class",
        Some(CAPABILITY_ROOT),
    );
    publish_playbook(&h, OTHER_TENANT, "playbook:other", "service_down", None);

    let batch = decide(&h, triage_request(&schema, TENANT)).await.unwrap();
    // One option per proposed step, by playbook id then step order; the
    // resource-pressure playbook has no trigger in the burst.
    assert_eq!(
        options(&batch),
        [
            "step:playbook:down:d1",
            "step:playbook:flap:s1",
            "step:playbook:flap:s2",
            "step:playbook:ticket:t1",
        ]
        .map(|id| (id.to_string(), STEP_SCORE))
        .to_vec()
    );
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    // The write step is named with its governed route; nothing is run.
    assert!(record.premises.iter().any(|p| {
        p.class == PremiseClass::Claim
            && p.fact
                .starts_with("step:playbook:flap:s2:writes=true:route=connector_write_back")
    }));
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("trigger:service_flapping:service=svc-a")));

    // Fewer state changes than the flapping threshold: the flapping playbook
    // is not proposed.
    let mut calm = triage_request(&schema, TENANT);
    let CandidateSource::Declared { options: declared } = &calm.candidates else {
        unreachable!()
    };
    let kept: Vec<DeclaredOption> = declared
        .iter()
        .filter(|o| o.option_id != "e3")
        .cloned()
        .collect();
    calm.candidates = CandidateSource::Declared {
        options: BoundedVec::new(kept).unwrap(),
    };
    let batch = decide(&h, calm).await.unwrap();
    assert!(options(&batch)
        .iter()
        .all(|(id, _)| !id.starts_with("step:playbook:flap:")));

    // An event without a service and kind is refused.
    let mut broken = triage_request(&schema, TENANT);
    broken.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![declared_option("e1")]).unwrap(),
    };
    assert!(decide(&h, broken).await.is_err());

    // Cross tenant: tenant B sees only its own playbook and none of A's.
    let other_schema = publish_route_schema_in(&h, OTHER_TENANT);
    let verified_other =
        VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let batch: DecisionBatch = decode(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &verified_other,
            triage_request(&other_schema, OTHER_TENANT),
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        options(&batch),
        vec![("step:playbook:other:o1".to_string(), STEP_SCORE)]
    );
}

fn execute_param(value: bool) -> Vec<TypedParam> {
    vec![TypedParam {
        name: PARAM_EXECUTE.to_string(),
        value: TypedValue::Bool(value),
    }]
}

// spec: MEDULLA-DECISION-ENGINE-R704.4
#[tokio::test]
async fn playbook_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_playbook(&h, TENANT, "playbook:flap", "service_flapping", None);

    // A request to execute is refused with the registered code and nothing is
    // sealed; asking not to execute is the ordinary evaluation.
    let mut execute = triage_request(&schema, TENANT);
    execute.params = BoundedVec::new(execute_param(true)).unwrap();
    let refused = decide(&h, execute).await.unwrap_err();
    assert!(refused.contains("PLAYBOOK_EVALUATE_ONLY"), "{refused}");
    let mut evaluate = triage_request(&schema, TENANT);
    evaluate.params = BoundedVec::new(execute_param(false)).unwrap();
    let batch = decide(&h, evaluate).await.unwrap();
    // The write step is named with the typed reason and is never run: the
    // answer is an advisory list of options.
    let record = &batch.records.as_slice()[0];
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert!(record.premises.iter().any(|p| {
        p.fact.starts_with("step:playbook:flap:s2:writes=true:")
            && p.fact.contains(":reason=EvaluateOnly:")
    }));

    // A burst whose trigger no playbook answers abstains and names the kind.
    let mut down = triage_request(&schema, TENANT);
    down.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![event("e4", "svc-b", "service_down", &[], 2_500)]).unwrap(),
    };
    let batch = decide(&h, down).await.unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("expected an abstention");
    };
    let AbstainReason::Family { code, detail, .. } = &reasons.as_slice()[0] else {
        panic!("expected a family abstention");
    };
    assert_eq!(code.as_str(), "PLAYBOOK_NO_TRIGGER");
    assert!(detail.contains("service_down"), "{detail}");

    // A burst that raises no trigger at all abstains the same way.
    let mut quiet = triage_request(&schema, TENANT);
    quiet.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![event("e1", "svc-a", "state_change", &[], 0)]).unwrap(),
    };
    let batch = decide(&h, quiet).await.unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("expected an abstention");
    };
    let AbstainReason::Family { code, detail, .. } = &reasons.as_slice()[0] else {
        panic!("expected a family abstention");
    };
    assert_eq!(code.as_str(), "PLAYBOOK_NO_TRIGGER");
    assert!(detail.contains("no trigger"), "{detail}");

    // Another tenant's playbook never answers this tenant's burst.
    publish_playbook(&h, OTHER_TENANT, "playbook:other", "service_down", None);
    let mut down = triage_request(&schema, TENANT);
    down.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![event("e4", "svc-b", "service_down", &[], 2_500)]).unwrap(),
    };
    let batch = decide(&h, down).await.unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R704.5
#[tokio::test]
async fn proposal_records_playbook_id_and_revision() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_playbook(&h, TENANT, "playbook:flap", "service_flapping", None);
    publish_playbook(&h, TENANT, "playbook:down", "service_down", None);

    let batch = decide(&h, triage_request(&schema, TENANT)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    // One Definition premise per proposing playbook, bound to its publisher
    // and carrying the revision it was read at.
    for id in ["playbook:flap", "playbook:down"] {
        let premise = record
            .premises
            .iter()
            .find(|p| p.fact.starts_with(&format!("playbook:{id}:revision=")))
            .unwrap_or_else(|| panic!("no definition premise for {id}"));
        assert_eq!(premise.class, PremiseClass::Definition);
        assert!(matches!(
            &premise.provenance,
            PremiseProvenance::Publisher { component_id, definition_digest }
                if component_id == id && !definition_digest.is_empty()
        ));
        assert!(premise.fact.ends_with("revision=1"), "{}", premise.fact);
    }
    // Each correlated event is a Claim: the three state changes of the
    // flapping service and the service-down event.
    for id in ["e1", "e2", "e3", "e4"] {
        assert!(
            record
                .premises
                .iter()
                .any(|p| p.class == PremiseClass::Claim
                    && p.fact.starts_with(&format!("event:{id}:"))),
            "no claim premise for {id}"
        );
    }
    // An event that raised nothing is not cited.
    assert!(!record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("event:e5:")));
    // The weakest class is the claim.
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
}

// spec: MEDULLA-DECISION-ENGINE-R704.1
// spec: MEDULLA-DECISION-ENGINE-R704.6
#[tokio::test]
async fn flapping_service_proposes_without_acting() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_playbook(&h, TENANT, "playbook:flap", "service_flapping", None);
    publish_playbook(&h, OTHER_TENANT, "playbook:other", "service_flapping", None);

    // Three state changes of one service inside the window are a flapping
    // service.
    let burst = |schema: &ComponentDependency, tenant: &str| {
        let mut request = triage_request(schema, tenant);
        request.candidates = CandidateSource::Declared {
            options: BoundedVec::new(vec![
                event("e1", "svc-a", "state_change", &[], 0),
                event("e2", "svc-a", "state_change", &[], 1_000),
                event("e3", "svc-a", "state_change", &[], 2_000),
            ])
            .unwrap(),
        };
        request
    };
    let batch = decide(&h, burst(&schema, TENANT)).await.unwrap();
    // The answer is an advisory list of proposed steps; nothing is run.
    assert_eq!(
        options(&batch),
        ["step:playbook:flap:s1", "step:playbook:flap:s2"]
            .map(|id| (id.to_string(), STEP_SCORE))
            .to_vec()
    );
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    // The write step is only named, with the governed route and the typed
    // reason that the engine evaluates and never acts.
    assert!(record.premises.iter().any(|p| {
        p.fact
            .starts_with("step:playbook:flap:s2:writes=true:route=connector_write_back")
            && p.fact.contains(":reason=EvaluateOnly:")
    }));
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("trigger:service_flapping:service=svc-a")));
    for id in ["e1", "e2", "e3"] {
        assert!(
            record
                .premises
                .iter()
                .any(|p| p.class == PremiseClass::Claim
                    && p.fact.starts_with(&format!("event:{id}:")))
        );
    }

    // A request to execute is refused and nothing is sealed.
    let mut execute = burst(&schema, TENANT);
    execute.params = BoundedVec::new(execute_param(true)).unwrap();
    let refused = decide(&h, execute).await.unwrap_err();
    assert!(refused.contains("PLAYBOOK_EVALUATE_ONLY"), "{refused}");

    // Another tenant's flapping playbook is never proposed to this tenant,
    // and this tenant's is never proposed to the other.
    let other_schema = publish_route_schema_in(&h, OTHER_TENANT);
    let verified_other =
        VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let other_burst = burst(&other_schema, OTHER_TENANT);
    let other: DecisionBatch = decode(
        super::super::statistical::handle_decide(&h.state, 1, &verified_other, other_burst).await,
    )
    .unwrap();
    assert_eq!(
        options(&other),
        ["step:playbook:other:o1"]
            .map(|id| (id.to_string(), STEP_SCORE))
            .to_vec()
    );
}
