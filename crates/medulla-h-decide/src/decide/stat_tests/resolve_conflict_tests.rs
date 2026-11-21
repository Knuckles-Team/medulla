//! MEDULLA-DECISION-ENGINE-R828: a served `resolve_conflict` question picks
//! the higher `reliability x confidence` score over the margin knob, breaks a
//! close tie by the later `valid_from` only for overlapping windows with the
//! tie-break knob on, and abstains under each registered code otherwise. The
//! `ConflictDecider` seam returns the same sealed record.

use super::*;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{
    ConflictCandidate, ConflictCandidates, DeclaredOption,
};
use medulla_decision_types::decision::AbstainReason;
use medulla_server_api::api::conflict_decider::ConflictDecider;
use medulla_types::contract::BoundedVec;

fn side(reliability: f64, confidence: f64, valid_from: u64, valid_until: Option<u64>) -> ConflictCandidate {
    ConflictCandidate {
        source_reliability: reliability,
        confidence,
        valid_from,
        valid_until,
        tx_from: valid_from,
        evidence_refs: vec![format!("evidence:{valid_from}")],
    }
}

fn pair(a: ConflictCandidate, b: ConflictCandidate) -> ConflictCandidates {
    ConflictCandidates {
        entity: "person:42".into(),
        candidate_a: a,
        candidate_b: b,
    }
}

fn pinned(h: &Harness, id: &str, knobs: &[(&str, KnobValue)]) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    for (name, value) in knobs {
        policy.knobs.insert(KnobName::new(name), *value);
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(id, &policy),
    }
}

fn request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    conflict: ConflictCandidates,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "branch.merge.conflict".to_string(),
            kind: QuestionKind::ResolveConflict,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "resolve_conflict".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy,
        params: BoundedVec::new(vec![TypedParam {
            name: "conflict".to_string(),
            value: TypedValue::ResolveConflict(Box::new(conflict)),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

async fn ask(h: &Harness, request: DecideRequest) -> StatisticalDecisionRecord {
    decide(h, request).await.unwrap().records.as_slice()[0].clone()
}

fn winner(record: &StatisticalDecisionRecord) -> String {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores.as_slice()[0].option_id.clone(),
        other => panic!("a winner is advisory, got {other:?}"),
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

fn has_premise(record: &StatisticalDecisionRecord, prefix: &str) -> bool {
    record
        .premises
        .iter()
        .any(|premise| premise.fact.starts_with(prefix))
}

// spec: MEDULLA-DECISION-ENGINE-R828
#[tokio::test]
async fn winner_by_margin_tie_and_abstain_codes() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let strict = pinned(&h, "conflict-policy-strict", &[]);
    let recency = pinned(
        &h,
        "conflict-policy-recency",
        &[("conflict.recency_tiebreak", KnobValue::Integer(1))],
    );

    // Winner by margin: 0.81 against 0.25 clears the default 0.15 margin.
    let clear = pair(side(0.9, 0.9, 10, None), side(0.5, 0.5, 20, None));
    let record = ask(&h, request(&schema, strict.clone(), clear.clone())).await;
    assert_eq!(winner(&record), "candidate_a");
    for premise in ["conflict:score_a=", "conflict:score_b=", "knob:conflict.min_margin="] {
        assert!(has_premise(&record, premise), "missing premise {premise}");
    }
    let flipped = pair(clear.candidate_b.clone(), clear.candidate_a.clone());
    assert_eq!(
        winner(&ask(&h, request(&schema, strict.clone(), flipped)).await),
        "candidate_b"
    );

    // Tie inside the margin: the later valid_from wins only for overlapping
    // windows with the tie-break knob on.
    let close = pair(side(0.6, 1.0, 10, None), side(0.55, 1.0, 20, None));
    assert_eq!(
        winner(&ask(&h, request(&schema, recency.clone(), close.clone())).await),
        "candidate_b"
    );
    let unresolved = ask(&h, request(&schema, strict.clone(), close)).await;
    assert_eq!(abstained_code(&unresolved), "CONFLICT_UNRESOLVED");
    assert!(has_premise(&unresolved, "conflict:score_a="));
    let disjoint = pair(side(0.6, 1.0, 10, Some(15)), side(0.55, 1.0, 20, None));
    assert_eq!(
        abstained_code(&ask(&h, request(&schema, recency.clone(), disjoint)).await),
        "CONFLICT_UNRESOLVED"
    );

    // No pinned policy, and a nonfinite score.
    assert_eq!(
        abstained_code(&ask(&h, request(&schema, DecisionPolicyRef::Default, clear.clone())).await),
        "CONFLICT_POLICY_MISSING"
    );
    let nonfinite = pair(side(f64::NAN, 0.9, 10, None), side(0.5, 0.5, 20, None));
    assert_eq!(
        abstained_code(&ask(&h, request(&schema, strict.clone(), nonfinite)).await),
        "CONFLICT_NONFINITE"
    );

    // The seam the BranchMerge handler calls answers with the same decision.
    let seamed = super::super::stat_decide::ConflictDeciderSeam
        .decide_conflict(&h.state, &verified(), request(&schema, strict.clone(), clear.clone()))
        .await
        .unwrap();
    assert_eq!(winner(&seamed), "candidate_a");

    // Cross-tenant: a request for another tenant is refused before any decision.
    let mut foreign = request(&schema, strict, clear);
    foreign.tenant_id = "tenant-b".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}
