//! MEDULLA-DECISION-ENGINE-R134.106: a served `AnswerRanking` question ranks
//! candidates from the caller's pairwise claims with no verifier, seals the
//! ranking on the decision record and reads it back through `DecisionLog`
//! `Get`. Every refusal is a typed abstention.

use super::log_tests::log_op;
use super::*;
use medulla_decision_outcome_types::decision::answer_ranking::ANSWER_RANKING_SCHEMA;
use medulla_decision_types::decision::answer_ranking_request::{
    AnswerRankingRequest, PairwiseComparison, PairwisePreference,
};
use medulla_decision_types::decision::PremiseClass;

fn pair(first: &str, second: &str, preference: PairwisePreference) -> PairwiseComparison {
    PairwiseComparison {
        first: first.into(),
        second: second.into(),
        preference,
    }
}

/// Four candidates: `c` wins every comparison, `a` beats `b` and ties `d`,
/// `b` ties `d`, and `d` loses to `c`. Scores are c=6, a=3, d=2, b=1.
fn body() -> AnswerRankingRequest {
    use PairwisePreference::{First, Second, Tie};
    AnswerRankingRequest {
        candidate_ids: BoundedVec::new(vec!["d".into(), "c".into(), "b".into(), "a".into()])
            .unwrap(),
        comparisons: BoundedVec::new(vec![
            pair("a", "b", First),
            pair("a", "c", Second),
            pair("a", "d", Tie),
            pair("c", "b", First),
            pair("b", "d", Tie),
            pair("d", "c", Second),
        ])
        .unwrap(),
        verifier: None,
        retain_top_k: 2,
        max_revisions: 2,
    }
}

fn ranking_request(h: &Harness, body: AnswerRankingRequest, declared: &[&str]) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    let mut request = declared_request(
        &schema,
        TENANT,
        QuestionKind::AnswerRanking,
        "x",
        DecisionPolicyRef::Default,
        vec![TypedParam {
            name: "answer_ranking".to_string(),
            value: TypedValue::AnswerRanking(Box::new(body)),
        }],
    );
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(declared.iter().map(|id| declared_option(id)).collect()).unwrap(),
    };
    request
}

fn ids(body: &AnswerRankingRequest) -> Vec<&str> {
    body.candidate_ids.iter().map(String::as_str).collect()
}

fn scores(record: &StatisticalDecisionRecord) -> Vec<(String, i64)> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    scores
        .iter()
        .map(|s| (s.option_id.clone(), s.score.value >> 32))
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R134.106
#[tokio::test]
async fn served_ranking_round_trips_through_readback() {
    let h = Harness::new().await;
    let request = body();
    let batch = decide(&h, ranking_request(&h, request.clone(), &ids(&request)))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.question.kind, QuestionKind::AnswerRanking);
    assert_eq!(
        scores(&record),
        vec![
            ("c".to_string(), 6),
            ("a".to_string(), 3),
            ("d".to_string(), 2),
            ("b".to_string(), 1)
        ],
        "a deterministic global order, scores counting two per win and one per tie"
    );
    // Claim premises for the pairwise comparisons, one Definition for the order,
    // and the retained set and each revision action as recorded facts.
    let claims = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("answer_pairwise:"))
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 6);
    assert!(claims.iter().all(|p| p.class == PremiseClass::Claim));
    let order = record
        .premises
        .iter()
        .find(|p| p.fact.starts_with("answer_global_order:"))
        .expect("the global order premise");
    assert_eq!(order.class, PremiseClass::Definition);
    assert!(order.fact.contains("order=c,a,d,b"), "{}", order.fact);
    assert!(order.fact.contains("retained=c,a"), "{}", order.fact);
    assert!(order.fact.contains(ANSWER_RANKING_SCHEMA) || order.subject == ANSWER_RANKING_SCHEMA);
    let revisions = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("answer_revision:"))
        .map(|p| p.subject.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        revisions.into_iter().collect::<Vec<_>>(),
        vec!["b".to_string(), "d".to_string()],
        "revisions only for the two non-retained candidates"
    );

    // The caller commits the record, then reads the same bytes back by decision id.
    let committed: DecisionLogCommitted = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    let get = |tenant: &str| DecisionLogOp::Get {
        tenant_id: tenant.to_string(),
        record_id: committed.record_id.clone(),
    };
    let entry: Option<DecisionLogEntry> = decode(log_op(&h, "decider", get(TENANT)).await).unwrap();
    assert_eq!(*entry.unwrap().record, record, "equal record on readback");

    // A cross-tenant Get refuses.
    let refused =
        decode::<Option<DecisionLogEntry>>(log_op(&h, "decider", get("tenant-other")).await);
    assert!(refused.unwrap_err().starts_with("ACCESS_DENIED"));
}

fn abstained_with(record: &StatisticalDecisionRecord, code: &str) {
    assert!(
        matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
        "{:?}",
        record.outcome
    );
    assert!(
        format!("{:?}", record.outcome).contains(code),
        "{:?}",
        record.outcome
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.106
#[tokio::test]
async fn ranking_refusals_are_typed_abstentions_and_declared_must_match() {
    let h = Harness::new().await;
    let mut cases: Vec<(&str, AnswerRankingRequest)> = Vec::new();
    let mut named = body();
    named.verifier = Some("judge".into());
    cases.push(("ANSWER_RANKING_VERIFIER_NAMED", named));
    let mut bound = body();
    bound.max_revisions = 1;
    cases.push(("ANSWER_RANKING_REVISION_BOUND_EXCEEDED", bound));
    let mut retain = body();
    retain.retain_top_k = 0;
    cases.push(("ANSWER_RANKING_INVALID_RETAIN", retain));
    let mut unknown = body();
    unknown.comparisons = BoundedVec::new(
        unknown
            .comparisons
            .iter()
            .cloned()
            .chain([pair("a", "z", PairwisePreference::First)])
            .collect(),
    )
    .unwrap();
    cases.push(("ANSWER_RANKING_UNKNOWN_CANDIDATE", unknown));
    let mut incomplete = body();
    incomplete.comparisons =
        BoundedVec::new(incomplete.comparisons.iter().skip(1).cloned().collect()).unwrap();
    cases.push(("ANSWER_RANKING_INCOMPLETE_COMPARISONS", incomplete));
    let mut repeated = body();
    repeated.comparisons = BoundedVec::new(
        repeated
            .comparisons
            .iter()
            .cloned()
            .chain([pair("a", "b", PairwisePreference::Second)])
            .collect(),
    )
    .unwrap();
    cases.push(("ANSWER_RANKING_DUPLICATE_COMPARISON", repeated));
    let mut duplicated = body();
    duplicated.candidate_ids =
        BoundedVec::new(vec!["a".into(), "a".into(), "b".into(), "c".into()]).unwrap();
    cases.push(("ANSWER_RANKING_DUPLICATE_CANDIDATE", duplicated));
    for (code, request) in cases {
        let declared: Vec<&str> = ids(&request);
        let batch = decide(&h, ranking_request(&h, request.clone(), &declared))
            .await
            .unwrap();
        abstained_with(&batch.records.as_slice()[0], code);
    }
    let mut few = body();
    few.candidate_ids = BoundedVec::new(vec!["a".into()]).unwrap();
    few.comparisons = BoundedVec::default();
    let batch = decide(&h, ranking_request(&h, few, &["a"])).await.unwrap();
    abstained_with(
        &batch.records.as_slice()[0],
        "ANSWER_RANKING_TOO_FEW_CANDIDATES",
    );

    // Declared candidates that differ from the ranking's candidate_ids refuse.
    let request = body();
    let refused = decide(&h, ranking_request(&h, request, &["a", "b", "c", "x"]))
        .await
        .unwrap_err();
    assert!(refused.contains("PARAMETER_INVALID"), "{refused}");
}
