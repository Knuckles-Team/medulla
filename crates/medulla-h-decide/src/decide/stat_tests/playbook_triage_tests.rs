//! The `playbook_triage` family through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R704.3): a burst of inbound events selects the
//! tenant's playbooks by trigger, the answer is an advisory list of proposed
//! steps in playbook then step order, a write step is named with its governed
//! route and never run, and one tenant never sees another tenant's playbooks.

use super::*;
use medulla_agent_types::agent_ontology::CAPABILITY_ROOT;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::PremiseClass;
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
