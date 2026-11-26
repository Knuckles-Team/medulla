//! The `revert_or_fix_forward` family through the actual Decide path
//! (MEDULLA-DECISION-ENGINE-R827): `Revert` is illegal when the culprit has
//! dependents, and the legal set is ranked.

use std::time::Instant;

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalDecisionRecord,
    StatisticalOutcome, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_types::contract::BoundedVec;

use super::super::stat_executor::ExecutionContext;
use super::*;

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn option(id: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    }
}

fn request(
    pin: &ComponentDependency,
    options: &[&str],
    dependents: &str,
    verdicts: &str,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "scm.revert".to_string(),
            kind: QuestionKind::RevertOrFixForward,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options.iter().map(|id| option(id)).collect()).unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            text("culprit", "cs-1"),
            text("dependents", dependents),
            text("verdicts", verdicts),
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

fn decide(
    h: &Harness,
    request: &DecideRequest,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
        defect_triage: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        None,
        0,
        "decider".to_string(),
        Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

/// The ranked option ids of an advisory answer, best first, or the abstention.
fn ranked(record: &StatisticalDecisionRecord) -> Result<Vec<String>, String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let mut scored: Vec<_> = scores.iter().collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.score.value));
            Ok(scored.iter().map(|s| s.option_id.clone()).collect())
        }
        StatisticalOutcome::Abstained { reasons } => Err(format!("{:?}", reasons.as_slice()[0])),
        other => panic!("unexpected outcome {other:?}"),
    }
}

const FAILED: &str = r#"[{"gate_id":"unit","verdict":"fail"},{"gate_id":"lint","verdict":"pass"}]"#;

/// Verifies MEDULLA-DECISION-ENGINE-R827: dependents forbid revert; with none,
/// revert ranks first; a lone `revert` over dependents abstains.
// spec: MEDULLA-DECISION-ENGINE-R827
#[tokio::test]
async fn dependents_forbid_revert() {
    let h = Harness::new().await;
    let pin = publish_route_schema(&h).0;
    let both = ["fix_forward", "revert"];

    // No dependents: both are legal and revert ranks first.
    let free = decide(&h, &request(&pin, &both, "[]", FAILED)).unwrap();
    assert_eq!(ranked(&free).unwrap(), ["revert", "fix_forward"]);

    // A dependent: revert drops out and only fix_forward is answered.
    let held = decide(&h, &request(&pin, &both, r#"["cs-2"]"#, FAILED)).unwrap();
    assert_eq!(ranked(&held).unwrap(), ["fix_forward"]);

    // Only revert declared, and a dependent exists: the registered refusal.
    let only = decide(&h, &request(&pin, &["revert"], r#"["cs-2","cs-3"]"#, FAILED)).unwrap();
    let refusal = ranked(&only).unwrap_err();
    assert!(refusal.contains("REVERT_HAS_DEPENDENTS"), "{refusal}");

    // A culprit no recorded gate failed on is not a culprit.
    let green = r#"[{"gate_id":"unit","verdict":"pass"}]"#;
    assert!(decide(&h, &request(&pin, &both, "[]", green)).is_err());

    // An option outside the family is refused.
    assert!(decide(&h, &request(&pin, &["rebase"], "[]", FAILED)).is_err());
}
