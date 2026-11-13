//! `research_next` served through `Decide` (MEDULLA-DECISION-ENGINE-R705.3): the
//! declared frontier sources rank by expected information value under the
//! spend budget, the answer carries one `stop` option, and a caller of another
//! tenant is refused before any read.

use super::kernel_family_support::{decide_as_other_tenant, facts};
use super::path_preference_tests::pins;
use super::*;
use medulla_decision_types::decision::research_voi::{ResearchStop, STOP_OPTION_ID};
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use medulla_decision_types::decision::ResolutionKind;

const ONE: i64 = 1 << 32;

fn numbers(p_useful: i64, gain: i64, cost: i64, depth: i64) -> BoundedVec<DeclaredNumber, 32> {
    BoundedVec::new(
        [
            ("cost", cost),
            ("depth", depth),
            ("gain", gain),
            ("p_useful", p_useful),
        ]
        .into_iter()
        .map(|(key, q32)| DeclaredNumber {
            key: key.to_string(),
            q32,
        })
        .collect(),
    )
    .unwrap()
}

fn source(id: &str, subtask: &str, p_pct: i64, gain: i64, cost: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: numbers(ONE * p_pct / 100, gain * ONE, cost * ONE, ONE),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "subtask".to_string(),
            text: subtask.to_string(),
        }])
        .unwrap(),
    }
}

/// One million micro-units already spent.
fn spent(id: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec!["spent".to_string()]).unwrap(),
        numbers: numbers(0, 0, ONE, 0),
        texts: BoundedVec::default(),
    }
}

fn request(
    schema: &ComponentDependency,
    options: Vec<DeclaredOption>,
    budget_limit: i64,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "research.next".into(),
            kind: QuestionKind::ResearchNext,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "budget_limit".to_string(),
            value: TypedValue::Int(budget_limit),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn frontier() -> Vec<DeclaredOption> {
    vec![
        // Expected values 9, 4 and -0.9 (below the floor).
        source("s-a", "intro", 100, 10, 1),
        source("s-b", "method", 50, 10, 1),
        source("s-c", "method", 10, 1, 1),
        spent("spent-1"),
    ]
}

/// Verifies MEDULLA-DECISION-ENGINE-R705.3: the reads rank by expected value,
/// the walk stops at the value floor and a read that no longer fits the budget
/// ends the walk; another tenant is refused.
// spec: MEDULLA-DECISION-ENGINE-R705.3
#[tokio::test]
async fn decide_routes_research_next() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);

    let batch = decide(&h, request(&pins.0, frontier(), 3_000_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.question.kind, QuestionKind::ResearchNext);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!("research_next answers an advisory: {:?}", record.outcome);
    };
    assert!(!calibrated);
    let ranked: Vec<(&str, i64)> = scores
        .iter()
        .map(|s| (s.option_id.as_str(), s.score.value))
        .collect();
    assert_eq!(
        ranked,
        vec![("s-a", 9 * ONE), ("s-b", 4 * ONE), (STOP_OPTION_ID, ONE)]
    );
    let answer = record
        .answer
        .as_ref()
        .and_then(|a| a.research())
        .expect("the family answers a research walk");
    assert_eq!(answer.stop, ResearchStop::ValueFloor);
    assert_eq!(answer.spent_micro, 1_000_000);
    assert_eq!(answer.remaining_micro, 0);
    let subtasks: Vec<&str> = answer.reads.iter().map(|r| r.subtask.as_str()).collect();
    assert_eq!(subtasks, vec!["intro", "method"]);
    let facts = facts(&record);
    assert!(
        facts.contains(&"research:budget_limit=3000000"),
        "{facts:?}"
    );
    assert!(facts.contains(&"research:spent=1000000"), "{facts:?}");

    // Only half a unit is left after the spent million: the first read does
    // not fit, so the walk ends on the budget with no read.
    let batch = decide(&h, request(&pins.0, frontier(), 1_500_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    let answer = record.answer.as_ref().and_then(|a| a.research()).unwrap();
    assert_eq!(answer.stop, ResearchStop::Budget);
    assert!(answer.reads.is_empty());
    assert_eq!(answer.remaining_micro, 500_000);

    // A malformed question is refused: the reserved option id, a missing number.
    let mut reserved = frontier();
    reserved.push(source("stop", "intro", 100, 10, 1));
    reserved.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    assert!(decide(&h, request(&pins.0, reserved, 3_000_000))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    let mut missing = frontier();
    missing[0].numbers = BoundedVec::default();
    assert!(decide(&h, request(&pins.0, missing, 3_000_000))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));

    let error = decide_as_other_tenant(&h, request(&pins.0, frontier(), 3_000_000)).await;
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}
