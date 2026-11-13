//! MEDULLA-DECISION-ENGINE-R137.14: a served `ContextReplacement` question gates
//! replacing a raw context on a complete paired held-out comparison. Within the
//! tolerance the answer is `replace`; beyond it, or on any incomplete or
//! mismatched evidence, the raw context is retained.

use super::*;
use medulla_decision_types::decision::context_replacement::{
    PairedContextEvaluation, QueryVerdict,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicy, EvidenceClass};

const RAW: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const CANDIDATE: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const SET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn verdict(id: &str, raw: u64, candidate: u64) -> QueryVerdict {
    QueryVerdict {
        query_id: id.to_string(),
        raw_answer_digest: RAW.to_string(),
        candidate_answer_digest: CANDIDATE.to_string(),
        raw_score_micro: raw,
        candidate_score_micro: candidate,
    }
}

fn evaluation(verdicts: Vec<QueryVerdict>, expected: u32) -> PairedContextEvaluation {
    PairedContextEvaluation {
        held_out_set_id: "held-out".to_string(),
        held_out_set_revision: 3,
        held_out_set_digest: SET.to_string(),
        expected_queries: expected,
        raw_context_digest: RAW.to_string(),
        candidate_context_digest: CANDIDATE.to_string(),
        evaluator_revision: "eval-1".to_string(),
        metric_id: "exact-match".to_string(),
        verdicts: BoundedVec::new(verdicts).unwrap(),
    }
}

fn request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    evaluation: PairedContextEvaluation,
    pins: Vec<(&str, &str)>,
) -> DecideRequest {
    let mut params = vec![TypedParam {
        name: "paired_evaluation".to_string(),
        value: TypedValue::ContextReplacement(Box::new(evaluation)),
    }];
    params.extend(pins.into_iter().map(|(name, text)| TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(text.to_string()),
    }));
    params.sort_by(|a, b| a.name.cmp(&b.name));
    declared_request(
        schema,
        TENANT,
        QuestionKind::ContextReplacement,
        "replacement",
        policy,
        params,
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

// spec: MEDULLA-DECISION-ENGINE-R137.14
#[tokio::test]
async fn replacement_follows_tolerance() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let within = || {
        evaluation(
            vec![
                verdict("a", 900_000, 890_000),
                verdict("b", 800_000, 790_000),
            ],
            2,
        )
    };
    let beyond = || {
        evaluation(
            vec![
                verdict("a", 900_000, 800_000),
                verdict("b", 800_000, 700_000),
            ],
            2,
        )
    };

    // Within the default 0.02 tolerance the candidate replaces the raw context.
    let batch = decide(
        &h,
        request(&schema, DecisionPolicyRef::Default, within(), vec![]),
    )
    .await
    .unwrap();
    assert_eq!(answer(&batch), "replace");
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "context_replacement.raw_mean_micro=850000"));
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "knob:context.replacement_tolerance=0.020000"
            || p.subject == "context_replacement"));

    // Beyond the tolerance the raw context is retained.
    let batch = decide(
        &h,
        request(&schema, DecisionPolicyRef::Default, beyond(), vec![]),
    )
    .await
    .unwrap();
    assert_eq!(answer(&batch), "retain_raw");

    // A pinned tolerance of 0.12 makes the same beyond-default evidence replace.
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("context.replacement_tolerance"),
        KnobValue::DecimalMicros(120_000),
    );
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-tolerance", &policy),
    };
    let batch = decide(&h, request(&schema, pinned, beyond(), vec![]))
        .await
        .unwrap();
    assert_eq!(answer(&batch), "replace");

    // A query without a verdict retains the raw context under its registered code.
    let missing = evaluation(vec![verdict("a", 900_000, 900_000)], 2);
    let batch = decide(
        &h,
        request(&schema, DecisionPolicyRef::Default, missing, vec![]),
    )
    .await
    .unwrap();
    assert_eq!(answer(&batch), "abstained:PAIRED_VERDICTS_INCOMPLETE");
}

// spec: MEDULLA-DECISION-ENGINE-R137.14
#[tokio::test]
async fn incomplete_verdicts_retain_raw() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let one = || evaluation(vec![verdict("a", 900_000, 900_000)], 1);
    let ask = |evaluation: PairedContextEvaluation, pins: Vec<(&str, &str)>| {
        request(&schema, DecisionPolicyRef::Default, evaluation, pins)
    };

    // No verdicts at all, and a duplicated query, are incomplete.
    for incomplete in [
        evaluation(vec![], 1),
        evaluation(vec![verdict("a", 1, 1), verdict("a", 1, 1)], 2),
    ] {
        let batch = decide(&h, ask(incomplete, vec![])).await.unwrap();
        assert_eq!(answer(&batch), "abstained:PAIRED_VERDICTS_INCOMPLETE");
    }

    // A stale raw context or evaluator revision is a digest mismatch.
    for pins in [
        vec![("expected_raw_context_digest", CANDIDATE)],
        vec![("expected_evaluator_revision", "eval-0")],
    ] {
        let batch = decide(&h, ask(one(), pins)).await.unwrap();
        assert_eq!(answer(&batch), "abstained:PAIRED_DIGEST_MISMATCH");
    }
    let mut malformed = one();
    malformed.raw_context_digest = "not-a-digest".to_string();
    let batch = decide(&h, ask(malformed, vec![])).await.unwrap();
    assert_eq!(answer(&batch), "abstained:PAIRED_DIGEST_MISMATCH");

    // An unnamed metric or an out-of-range score leaves the metric undefined.
    let mut unnamed = one();
    unnamed.metric_id.clear();
    let batch = decide(&h, ask(unnamed, vec![])).await.unwrap();
    assert_eq!(answer(&batch), "abstained:PAIRED_METRIC_UNDEFINED");
    let out_of_range = evaluation(vec![verdict("a", 900_000, 1_000_001)], 1);
    let batch = decide(&h, ask(out_of_range, vec![])).await.unwrap();
    assert_eq!(answer(&batch), "abstained:PAIRED_METRIC_UNDEFINED");

    // The abstention keeps the raw revision and names it; nothing is written.
    let batch = decide(&h, ask(evaluation(vec![], 1), vec![]))
        .await
        .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("expected an abstention")
    };
    let AbstainReason::Family { detail, .. } = &reasons.as_slice()[0] else {
        panic!("expected a family abstention")
    };
    assert!(detail.contains("raw retained"), "{detail}");

    // A request without the evaluation is a malformed request, not an answer.
    let mut bare = ask(one(), vec![]);
    bare.params = BoundedVec::default();
    assert!(decide(&h, bare)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}
