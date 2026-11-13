//! MEDULLA-DECISION-ENGINE-R134.13: a served `AnalystSynthesis` question accounts
//! for every expected analyst, resolves each citation against the records the
//! caller's graph view shows, and abstains with no recommendation on any failure.

use super::*;
use medulla_decision_types::decision::analyst_synthesis::{
    AnalystAccount, AnalystFinding, AnalystGap, Citation, SynthesisRequest, SynthesisResult,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::AbstainReason;
use medulla_types::contract::BoundedVec;

const EVIDENCE: &str = "synth-evidence";
const FOREIGN_GRAPH: &str = "synth-foreign";
const PLAN: &str = "plan-digest-1";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A harness whose caller reads the evidence graph (`ev:1`, `ev:2`) and cannot
/// reach a second graph holding `ev:foreign`.
async fn evidence_harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        for (graph, nodes) in [
            (EVIDENCE, vec!["ev:1", "ev:2"]),
            (FOREIGN_GRAPH, vec!["ev:foreign"]),
        ] {
            guard
                .registry
                .create_graph(
                    graph,
                    medulla_protocol_types::protocol::GraphType::Team,
                    None,
                )
                .unwrap();
            let core = guard.registry.get(graph).unwrap().core.clone();
            for node in nodes {
                let props = serde_json::json!({"revision": 1, "content_digest": DIGEST});
                core.add_node(node.to_string(), rmp_serde::to_vec_named(&props).unwrap());
            }
        }
    }
    h
}

fn cite(evidence: &str) -> Citation {
    Citation {
        evidence_id: evidence.into(),
        revision: 1,
        digest: DIGEST.into(),
        span: "0..10".into(),
    }
}

fn finding(
    id: &str,
    analyst: &str,
    key: &str,
    claim: &str,
    cites: Vec<Citation>,
) -> AnalystFinding {
    AnalystFinding {
        finding_id: id.into(),
        analyst_id: analyst.into(),
        role: "analyst".into(),
        claim_key: key.into(),
        claim_digest: claim.into(),
        citations: cites,
    }
}

fn base() -> SynthesisRequest {
    SynthesisRequest {
        plan_digest: PLAN.into(),
        expected_analyst_ids: vec!["a1".into(), "a2".into(), "lead".into()],
        decision_role_id: "lead".into(),
        findings: vec![
            finding("f1", "a1", "revenue", "c-1", vec![cite("ev:1")]),
            finding("f2", "lead", "revenue", "c-1", vec![cite("ev:2")]),
        ],
        gaps: vec![AnalystGap {
            analyst_id: "a2".into(),
            reason: "timed out".into(),
        }],
    }
}

fn request(schema: &ComponentDependency, asked: &SynthesisRequest, plan: &str) -> DecideRequest {
    let mut params = vec![
        TypedParam {
            name: "synthesis".to_string(),
            value: TypedValue::Text(serde_json::to_string(asked).unwrap()),
        },
        TypedParam {
            name: "expected_plan_digest".to_string(),
            value: TypedValue::Text(plan.to_string()),
        },
        TypedParam {
            name: "source_graph".to_string(),
            value: TypedValue::Text(EVIDENCE.to_string()),
        },
    ];
    params.sort_by(|a, b| a.name.cmp(&b.name));
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "analyst.synthesis".to_string(),
            kind: QuestionKind::AnalystSynthesis,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "synthesis".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn result_of(record: &StatisticalDecisionRecord) -> SynthesisResult {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            serde_json::from_str(id.strip_prefix("synthesis:").expect("a synthesis answer"))
                .unwrap()
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn abstained_code(record: &StatisticalDecisionRecord) -> String {
    match &record.outcome {
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, .. } => code.as_str().to_string(),
            other => panic!("unexpected reason {other:?}"),
        },
        other => panic!("a refusal abstains, got {other:?}"),
    }
}

async fn ask(
    h: &Harness,
    schema: &ComponentDependency,
    asked: &SynthesisRequest,
) -> StatisticalDecisionRecord {
    decide(h, request(schema, asked, PLAN))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone()
}

// spec: MEDULLA-DECISION-ENGINE-R134.13
#[tokio::test]
async fn cited_compatible_findings_account_for_every_analyst() {
    let h = evidence_harness().await;
    let (schema, _) = publish_route_schema(&h);

    // A supported result that accounts for every analyst, the failed one included.
    let record = ask(&h, &schema, &base()).await;
    let result = result_of(&record);
    assert_eq!(result.decision_role_id, "lead");
    assert_eq!(result.conclusions.len(), 1);
    assert_eq!(result.conclusions[0].claim_key, "revenue");
    assert_eq!(result.conclusions[0].finding_ids, vec!["f1", "f2"]);
    assert_eq!(result.accounting.len(), 3);
    assert!(matches!(
        &result.accounting[1],
        AnalystAccount::Gap { analyst_id, .. } if analyst_id == "a2"
    ));
    let claims = record
        .premises
        .iter()
        .filter(|premise| premise.fact.starts_with("finding:analyst="))
        .count();
    assert_eq!(claims, 2, "one claim premise per finding");

    // Each refusal abstains under its registered code, with no recommendation.
    let mut missing = base();
    missing.gaps.clear();
    let mut duplicate = base();
    duplicate.gaps.push(AnalystGap {
        analyst_id: "a1".into(),
        reason: "also failed".into(),
    });
    let mut conflict = base();
    conflict.findings[1].claim_digest = "c-2".into();
    let mut uncited = base();
    uncited.findings[0].citations.clear();
    let mut role = base();
    role.decision_role_id = "chair".into();
    let mut bad_digest = base();
    bad_digest.findings[0].citations[0].digest = "stale".into();
    // An evidence record of a graph this question cannot see fails as INVALID_CITATION.
    let mut foreign = base();
    foreign.findings[0].citations = vec![cite("ev:foreign")];
    for (asked, code) in [
        (missing, "ANALYST_MISSING"),
        (duplicate, "ANALYST_DUPLICATE"),
        (conflict, "UNRESOLVED_CONFLICT"),
        (uncited, "MISSING_CITATION"),
        (role, "DECISION_ROLE_ABSENT"),
        (bad_digest, "INVALID_CITATION"),
        (foreign, "INVALID_CITATION"),
    ] {
        let record = ask(&h, &schema, &asked).await;
        assert_eq!(abstained_code(&record), code);
    }
    let mismatch = decide(&h, request(&schema, &base(), "another-plan"))
        .await
        .unwrap();
    assert_eq!(
        abstained_code(&mismatch.records.as_slice()[0]),
        "SYNTHESIS_PLAN_MISMATCH"
    );
}
