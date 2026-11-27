//! MEDULLA-DECISION-ENGINE-R106.1: a served `council_coordination` question
//! selects the option a quorum of pinned positions supports, abstains on a tie
//! or a thin majority, refuses each malformed request under its registered
//! code, and keeps every input position in the result.

use super::*;
use medulla_decision_types::decision::council::{
    CouncilPolicy, CouncilPosition, CouncilResult, EvidenceRef,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::AbstainReason;
use medulla_types::contract::BoundedVec;

fn head(id: &str) -> EvidenceRef {
    EvidenceRef {
        id: id.to_string(),
        revision: 4,
        digest: format!("sha256:{id}"),
    }
}

fn position(id: &str, persona: &str, option: &str) -> CouncilPosition {
    CouncilPosition {
        council_id: "council-1".to_string(),
        topology_revision: 9,
        position_id: id.to_string(),
        persona_id: persona.to_string(),
        persona_revision: 1,
        option_id: option.to_string(),
        claim_digest: format!("sha256:claim-{id}"),
        evidence_refs: vec![head("e1")],
        constraint_refs: vec!["constraint-1".to_string()],
    }
}

fn text<T: serde::Serialize>(name: &str, value: &T) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(serde_json::to_string(value).unwrap()),
    }
}

fn request(
    schema: &ComponentDependency,
    positions: &[CouncilPosition],
    quorum_micros: u32,
    heads: &[EvidenceRef],
) -> DecideRequest {
    let policy = CouncilPolicy {
        policy_id: "council-policy".to_string(),
        revision: 2,
        quorum_micros,
    };
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "council.coordinate".to_string(),
            kind: QuestionKind::CouncilCoordination,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "council".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "council_id".to_string(),
                value: TypedValue::Text("council-1".to_string()),
            },
            TypedParam {
                name: "topology_revision".to_string(),
                value: TypedValue::Int(9),
            },
            text("positions", &positions),
            text("council_policy", &policy),
            text("evidence_current", &heads),
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

fn selected(record: &StatisticalDecisionRecord) -> CouncilResult {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let answer = &scores.as_slice()[0].option_id;
            let json = answer
                .strip_prefix("council_result:")
                .expect("a result answer");
            serde_json::from_str(json).unwrap()
        }
        other => panic!("a selection is advisory, got {other:?}"),
    }
}

fn abstained_code(record: &StatisticalDecisionRecord) -> String {
    match &record.outcome {
        StatisticalOutcome::Abstained { reasons } => match &reasons.as_slice()[0] {
            AbstainReason::Family { code, .. } => code.as_str().to_string(),
            other => panic!("unexpected reason {other:?}"),
        },
        other => panic!("an abstention was expected, got {other:?}"),
    }
}

async fn ask(h: &Harness, request: DecideRequest) -> StatisticalDecisionRecord {
    decide(h, request).await.unwrap().records.as_slice()[0].clone()
}

// spec: MEDULLA-DECISION-ENGINE-R106.1
#[tokio::test]
async fn council_quorum_conflict_and_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let heads = [head("e1")];

    // A majority above the quorum selects its option and keeps every input.
    let majority = [
        position("p1", "persona-a", "ship"),
        position("p2", "persona-b", "ship"),
        position("p3", "persona-c", "hold"),
    ];
    let record = ask(&h, request(&schema, &majority, 500_000, &heads)).await;
    let result = selected(&record);
    assert_eq!(result.input_position_ids, ["p1", "p2", "p3"]);
    assert_eq!(result.accepted_position_ids, ["p1", "p2"]);
    assert_eq!(result.rejected_position_ids, ["p3"]);
    for prefix in [
        "position:p1",
        "evidence:e1@4",
        "council_policy:council-policy@2",
    ] {
        assert!(
            record.premises.iter().any(|p| p.fact == prefix),
            "missing premise {prefix}"
        );
    }

    // Two options with equal support abstain.
    let tie = [
        position("p1", "persona-a", "ship"),
        position("p2", "persona-b", "hold"),
    ];
    let record = ask(&h, request(&schema, &tie, 500_000, &heads)).await;
    assert_eq!(abstained_code(&record), "COUNCIL_UNRESOLVED_CONFLICT");

    // A best option below the policy's quorum abstains.
    let record = ask(&h, request(&schema, &majority, 900_000, &heads)).await;
    assert_eq!(abstained_code(&record), "COUNCIL_NO_QUORUM");

    // Each malformed request is refused under its registered code.
    let refused = |request: DecideRequest, code: &'static str| {
        let h = &h;
        async move {
            let error = decide(h, request).await.unwrap_err();
            assert!(error.starts_with(&format!("{code}: ")), "{code}: {error}");
        }
    };
    refused(
        request(&schema, &[], 500_000, &heads),
        "COUNCIL_INCOMPLETE_INPUT",
    )
    .await;
    let mut other_council = majority.clone();
    other_council[0].council_id = "council-2".to_string();
    refused(
        request(&schema, &other_council, 500_000, &heads),
        "COUNCIL_INCOMPLETE_INPUT",
    )
    .await;
    let mut duplicate = majority.clone();
    duplicate[1].persona_id = "persona-a".to_string();
    refused(
        request(&schema, &duplicate, 500_000, &heads),
        "COUNCIL_DUPLICATE_POSITION",
    )
    .await;
    let mut moved = majority.clone();
    moved[2].topology_revision = 10;
    refused(
        request(&schema, &moved, 500_000, &heads),
        "COUNCIL_TOPOLOGY_MISMATCH",
    )
    .await;
    refused(
        request(&schema, &majority, 500_000, &[]),
        "COUNCIL_MISSING_EVIDENCE",
    )
    .await;
    let mut changed = head("e1");
    changed.digest = "sha256:changed".to_string();
    refused(
        request(&schema, &majority, 500_000, &[changed]),
        "COUNCIL_STALE_EVIDENCE",
    )
    .await;
    refused(
        request(&schema, &majority, 400_000, &heads),
        "COUNCIL_POLICY_REFUSED",
    )
    .await;

    // Cross-tenant: a request for another tenant is refused before any decision.
    let mut foreign = request(&schema, &majority, 500_000, &heads);
    foreign.tenant_id = "tenant-b".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}
