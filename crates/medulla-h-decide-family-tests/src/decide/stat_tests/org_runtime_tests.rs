//! MEDULLA-ORCH-VOCAB-R750.751: a served `recruit` question ranks a role's
//! employees by capability coverage (ties on the lowest agent IRI), allocates
//! the top legal market offer with its priority bucket while every excluded
//! Gap keeps its `OfferExclusion` code, replays a twin against its pinned
//! revisions on the verify-replay path, and an instance moves on the
//! `AGENT_INSTANCE_CHART`.

use std::collections::BTreeSet;

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_types::contract::BoundedVec;
use medulla_work_types::orchestration::org::{
    AgentTwin, EmployeeSpec, RoleSpec, AGENT_INSTANCE_CHART,
};
use medulla_work_types::work_market::selection::{MarketFacts, OfferExclusion};
use medulla_work_types::work_market::{
    BlastRadius, GapEvidenceInput, GapUpsertRequest, GapView, GapWorkSpec, WorkOffer, WorkOfferView,
};
use {
    medulla_decision_program_types::decision::recruit::RecruitCandidate,
    medulla_decision_program_types::decision::recruit::RecruitMarket,
    medulla_decision_program_types::decision::recruit::RecruitRequest,
    medulla_decision_program_types::decision::recruit::RecruitResult,
};

const CODE: &str = "eg:capability/code";
const REVIEW: &str = "eg:capability/review";

fn candidate(agent: &str, revision: &str, capabilities: &[&str]) -> RecruitCandidate {
    RecruitCandidate {
        employee: EmployeeSpec {
            agent_ref: agent.into(),
            definition_revision: revision.into(),
            role_id: "role:builder".into(),
        },
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
    }
}

fn offer(utility: u64, cost: u64) -> WorkOffer {
    WorkOffer {
        expected_utility_micros: utility,
        probability_of_closure_ppm: 500_000,
        expected_cost_microunits: cost,
        cost_uncertainty_ppm: 100_000,
        blast_radius: BlastRadius::Repository,
        reversible: true,
        required_capabilities: vec![CODE.into()],
        repository_scope: Vec::new(),
        cooldown_until_ms: 0,
        depends_on_gap_ids: Vec::new(),
        evidence_digests: vec![format!("sha256:{}", "ab".repeat(32))],
    }
}

fn gap(gap_id: &str, created_at_ms: u64, priced: WorkOffer) -> GapView {
    let request = GapUpsertRequest {
        tenant: TENANT.into(),
        gap_id: gap_id.into(),
        source: "failure".into(),
        signature: gap_id.into(),
        statement: "work to allocate".into(),
        domain: "runtime".into(),
        severity_ppm: 700_000,
        concept_ids: Vec::new(),
        evidence: vec![GapEvidenceInput {
            digest: format!("sha256:{}", "ab".repeat(32)),
            kind: "failure_cluster".into(),
            reference: "cluster:1".into(),
        }],
        work: GapWorkSpec {
            kind: "gap_remediation".into(),
            max_attempts: 3,
        },
        idempotency_key: format!("upsert:{gap_id}"),
    };
    let mut view = request.fresh_gap(format!("wi:{gap_id}"), created_at_ms);
    view.offer = Some(WorkOfferView {
        version: 1,
        generation: view.generation,
        work_item_id: view.work_item_id.clone(),
        offer: priced,
        utility_rate: 0,
        stage: "eg/work-offer-utility-rate/v1".into(),
        offered_at_ms: created_at_ms,
    });
    view.offer_version = 1;
    view
}

fn market() -> RecruitMarket {
    let mut blocked = offer(9_000_000, 2_000);
    blocked.depends_on_gap_ids = vec!["gap:open".into()];
    RecruitMarket {
        gaps: vec![
            gap("gap:low", 1, offer(1_000_000, 2_000)),
            gap("gap:best", 2, offer(4_000_000, 2_000)),
            gap("gap:blocked", 0, blocked),
            gap("gap:over", 0, offer(9_000_000, 50_000)),
        ],
        facts: MarketFacts {
            now_ms: 1_000,
            resolved_gap_ids: BTreeSet::new(),
            available_capabilities: BTreeSet::from([CODE.to_string()]),
            repository_scope: None,
            max_blast_radius: BlastRadius::Repository,
            attempt_budget_microunits: 10_000,
            cost_floor_microunits: 1_000,
        },
    }
}

fn asked(twin: Option<AgentTwin>) -> RecruitRequest {
    RecruitRequest {
        role: RoleSpec {
            role_id: "role:builder".into(),
            required_capabilities: vec![CODE.into(), REVIEW.into()],
            headcount: 1,
        },
        candidates: vec![
            candidate("agent:c", "rev-1", &[CODE]),
            candidate("agent:b", "rev-2", &[CODE, REVIEW]),
            candidate("agent:a", "rev-2", &[CODE, REVIEW]),
            candidate("agent:a", "rev-1", &[CODE]),
        ],
        market: Some(market()),
        twin,
    }
}

fn request(schema: &ComponentDependency, asked: &RecruitRequest) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "org.recruit".to_string(),
            kind: QuestionKind::Recruit,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "recruit".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "recruit".to_string(),
            value: TypedValue::Text(serde_json::to_string(asked).unwrap()),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

fn result_of(record: &StatisticalDecisionRecord) -> RecruitResult {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            serde_json::from_str(id.strip_prefix("recruit:").expect("a recruit answer")).unwrap()
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn ranked(result: &RecruitResult) -> Vec<(&str, &str, u32)> {
    result
        .ranked
        .iter()
        .map(|e| {
            (
                e.agent_ref.as_str(),
                e.definition_revision.as_str(),
                e.covered,
            )
        })
        .collect()
}

// spec: MEDULLA-ORCH-VOCAB-R750.751
#[tokio::test]
async fn recruit_allocate_and_twin_replay() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Recruit: full coverage first, ties on the lowest agent IRI.
    let record = decide(&h, request(&schema, &asked(None)))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    let result = result_of(&record);
    assert_eq!(result.role_id, "role:builder");
    assert_eq!(
        ranked(&result),
        vec![
            ("agent:a", "rev-2", 2),
            ("agent:b", "rev-2", 2),
            ("agent:a", "rev-1", 1),
            ("agent:c", "rev-1", 1),
        ]
    );

    // Allocate: only the top legal offer, with its priority bucket; every
    // excluded Gap keeps its exclusion code.
    let allocation = result.allocation.as_ref().expect("a market was given");
    assert_eq!(allocation.ranked.len(), 1);
    let top = allocation.top().unwrap();
    assert_eq!(top.gap_id, "gap:best");
    assert_eq!(top.work_item_id, "wi:gap:best");
    assert_eq!(top.priority_bucket, market().gaps[1].priority_bucket);
    assert_eq!(
        allocation.excluded,
        vec![
            ("gap:blocked".to_string(), OfferExclusion::DependencyOpen),
            ("gap:over".to_string(), OfferExclusion::OverBudget),
        ]
    );
    assert_eq!(OfferExclusion::DependencyOpen.code(), "DEPENDENCY_OPEN");
    assert!(allocation
        .excluded
        .iter()
        .all(|(gap_id, _)| gap_id != &top.gap_id));

    // Twin: `agent:a` is replayed at its pinned `rev-1` only.
    let twin = AgentTwin::new("agent:a", vec!["rev-1".into()]).unwrap();
    let twin_record = decide(&h, request(&schema, &asked(Some(twin))))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert_eq!(
        ranked(&result_of(&twin_record)),
        vec![
            ("agent:b", "rev-2", 2),
            ("agent:a", "rev-1", 1),
            ("agent:c", "rev-1", 1),
        ]
    );
    let ctx = medulla_h_decide_base::decide::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1,
        server_secret: b"fixture",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
        defect_triage: None,
    };
    for sealed in [&record, &twin_record] {
        let replayed = medulla_h_decide::decide::stat_replay_hi::replay(&ctx, sealed)
            .expect("the replay re-derives the same answer from the stored request");
        assert_eq!(replayed.record_digest, sealed.record_digest);
    }

    // Instances move on the agent-instance chart; an unknown move does not fire.
    use medulla_statechart::context::{Context, EventInput};
    use medulla_statechart::{transition, validate};
    let chart = medulla_store::loop_statechart::agent_instance_statechart_def();
    validate(&chart).expect("the agent-instance chart is structurally valid");
    assert_eq!(chart.name, AGENT_INSTANCE_CHART);
    let context = Context::new();
    let mut state = chart.initial.clone();
    for (event, next) in [
        ("activate", "active"),
        ("drain", "draining"),
        ("terminate", "terminated"),
    ] {
        let moved = transition(&chart, &state, &context, &EventInput::new(event)).unwrap();
        assert!(moved.fired);
        assert_eq!(moved.next_state, next);
        state = moved.next_state;
    }
    assert!(chart.is_final(&state));
    let refused = transition(&chart, "active", &context, &EventInput::new("activate"));
    assert!(refused.map(|out| !out.fired).unwrap_or(true));
}

// spec: MEDULLA-ORCH-VOCAB-R750.752
#[tokio::test]
async fn pinless_twin_refuses_and_excluded_offer_has_no_top() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Fault 1: a twin whose every pin is blank refuses the request.
    let mut body = serde_json::to_value(asked(None)).unwrap();
    body["twin"] = serde_json::json!({ "agent_ref": "agent:a", "pinned_revisions": [""] });
    let mut pinless = request(&schema, &asked(None));
    pinless.params = BoundedVec::new(vec![TypedParam {
        name: "recruit".to_string(),
        value: TypedValue::Text(body.to_string()),
    }])
    .unwrap();
    let refused = decide(&h, pinless).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(refused.contains("TWIN_PIN_MISSING"), "{refused}");

    // Fault 2: when every offer is excluded there is no top offer, and each
    // excluded gap keeps its exclusion code.
    let mut excluded = asked(None);
    let market = excluded.market.as_mut().expect("a market was given");
    market
        .gaps
        .retain(|gap| gap.gap_id == "gap:blocked" || gap.gap_id == "gap:over");
    let record = decide(&h, request(&schema, &excluded))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    let allocation = result_of(&record).allocation.expect("a market was given");
    assert!(allocation.top().is_none());
    assert_eq!(
        allocation.excluded,
        vec![
            ("gap:blocked".to_string(), OfferExclusion::DependencyOpen),
            ("gap:over".to_string(), OfferExclusion::OverBudget),
        ]
    );
}
