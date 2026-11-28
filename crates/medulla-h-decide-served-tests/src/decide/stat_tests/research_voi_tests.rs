//! `research_next` served through `Decide` (MEDULLA-DECISION-ENGINE-R705.3): the
//! declared frontier sources rank by expected information value under the
//! spend budget, the answer carries one `stop` option, and a caller of another
//! tenant is refused before any read.

use medulla_decision_types::decision::record::ResolutionKind;
use medulla_h_decide_stat_tests::decide::stat_tests::path_preference_tests::pins;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::research_voi::ResearchStop,
    medulla_decision_statistical_types::decision::research_voi::RESEARCH_DEPTH_EXCEEDED,
    medulla_decision_statistical_types::decision::research_voi::RESEARCH_NO_BUDGET,
    medulla_decision_statistical_types::decision::research_voi::STOP_OPTION_ID,
};
use {
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredNumber,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredText,
};
use {
    medulla_h_decide_stat_tests::decide::stat_tests::kernel_family_support::decide_as_other_tenant,
    medulla_h_decide_stat_tests::decide::stat_tests::kernel_family_support::facts,
};

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

/// One million micro-units already spent by `subtask`.
fn spent(id: &str, subtask: &str) -> DeclaredOption {
    spent_units(id, subtask, 1)
}

fn spent_units(id: &str, subtask: &str, units: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec!["spent".to_string()]).unwrap(),
        numbers: numbers(0, 0, units * ONE, 0),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "subtask".to_string(),
            text: subtask.to_string(),
        }])
        .unwrap(),
    }
}

/// The declared budget of `subtask`, in whole units.
fn budget(id: &str, subtask: &str, units: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec!["subtask".to_string()]).unwrap(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "budget".to_string(),
            q32: units * ONE,
        }])
        .unwrap(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "subtask".to_string(),
            text: subtask.to_string(),
        }])
        .unwrap(),
    }
}

fn family_abstention(outcome: &StatisticalOutcome) -> (String, String) {
    let StatisticalOutcome::Abstained { reasons } = outcome else {
        panic!("expected an abstention, got {outcome:?}");
    };
    match reasons.as_slice() {
        [AbstainReason::Family { code, detail, .. }] => (code.as_str().to_string(), detail.clone()),
        other => panic!("expected one family reason, got {other:?}"),
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
        evidence_streams: Default::default(),
    }
}

fn frontier() -> Vec<DeclaredOption> {
    vec![
        // Expected values 9, 4 and -0.9 (below the floor).
        source("s-a", "intro", 100, 10, 1),
        source("s-b", "method", 50, 10, 1),
        source("s-c", "method", 10, 1, 1),
        spent("spent-1", "intro"),
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
    let StatisticalOutcome::Advisory {
        scores, calibrated, ..
    } = &record.outcome
    else {
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
    // The served tenant boundary refuses a request naming a tenant the
    // caller does not belong to, before any pin is read.
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R705.4: a subtask with no budget left and
/// a node past the depth limit each abstain with their registered code, the
/// first naming the subtask; nothing is written and another tenant is refused.
// spec: MEDULLA-DECISION-ENGINE-R705.4
#[tokio::test]
async fn research_refusals() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);

    // "intro" declared 1 unit and has already spent it.
    let mut broke = frontier();
    broke.push(budget("b-intro", "intro", 1));
    broke.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let batch = decide(&h, request(&pins.0, broke, 3_000_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let (code, detail) = family_abstention(&record.outcome);
    assert_eq!(code, RESEARCH_NO_BUDGET.as_str());
    assert!(detail.contains("intro"), "{detail}");

    // A node four links deep is past the default depth limit of three.
    let mut deep = frontier();
    deep[0].numbers = numbers(ONE, 10 * ONE, ONE, 4 * ONE);
    let batch = decide(&h, request(&pins.0, deep, 3_000_000)).await.unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let (code, detail) = family_abstention(&record.outcome);
    assert_eq!(code, RESEARCH_DEPTH_EXCEEDED.as_str());
    assert!(detail.contains("s-a"), "{detail}");

    // A subtask with budget left is not an abstention.
    let mut funded = frontier();
    funded.push(budget("b-intro", "intro", 5));
    funded.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let batch = decide(&h, request(&pins.0, funded, 9_000_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);

    let error = decide_as_other_tenant(&h, request(&pins.0, frontier(), 3_000_000)).await;
    // The served tenant boundary refuses a request naming a tenant the
    // caller does not belong to, before any pin is read.
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R705.5: the sealed record carries one
/// claim premise per reported spent group with its subtask, the budget limit
/// and the remaining budget of every declared subtask after the reads.
// spec: MEDULLA-DECISION-ENGINE-R705.5
#[tokio::test]
async fn spent_units_are_recorded_per_subtask() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);

    let mut options = frontier();
    options.push(spent_units("spent-2", "method", 2));
    options.push(budget("b-intro", "intro", 4));
    options.push(budget("b-method", "method", 4));
    options.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let batch = decide(&h, request(&pins.0, options, 9_000_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let facts = facts(&record);
    assert!(
        facts.contains(&"research:budget_limit=9000000"),
        "{facts:?}"
    );
    assert!(facts.contains(&"research:spent=3000000"), "{facts:?}");
    assert!(facts.contains(&"spent:intro=1000000"), "{facts:?}");
    assert!(facts.contains(&"spent:method=2000000"), "{facts:?}");
    // Reads: s-a (intro, cost 1) and s-b (method, cost 1) fit; s-c is under
    // the value floor. intro: 4 - 1 - 1; method: 4 - 2 - 1.
    assert!(facts.contains(&"remaining:intro=2000000"), "{facts:?}");
    assert!(facts.contains(&"remaining:method=1000000"), "{facts:?}");
    // Every ledger premise the research family states is a caller claim; the
    // sealing premises every record carries (context, schema, policy) keep
    // their own Definition class.
    let ledger: Vec<_> = record
        .premises
        .iter()
        .filter(|premise| premise.subject == "research_next")
        .collect();
    assert!(ledger.len() >= 6, "{ledger:?}");
    for premise in ledger {
        assert_eq!(premise.class, PremiseClass::Claim, "{premise:?}");
    }

    // A spent group that names no subtask is refused.
    let mut nameless = frontier();
    nameless[3].texts = BoundedVec::default();
    assert!(decide(&h, request(&pins.0, nameless, 9_000_000))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));

    let error = decide_as_other_tenant(&h, request(&pins.0, frontier(), 9_000_000)).await;
    // The served tenant boundary refuses a request naming a tenant the
    // caller does not belong to, before any pin is read.
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R705.6
#[tokio::test]
async fn budget_stops_at_marginal_value() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);

    // A frontier and a budget: the reads follow the ranking and the walk reports
    // its spent units, then observes `stop` once the best value reaches the floor.
    let batch = decide(&h, request(&pins.0, frontier(), 3_000_000))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let answer = record
        .answer
        .as_ref()
        .and_then(|a| a.research())
        .expect("the family answers a research walk");
    assert_eq!(answer.stop, ResearchStop::ValueFloor);
    assert_eq!(answer.spent_micro, 1_000_000);
    assert_eq!(
        answer
            .reads
            .iter()
            .map(|r| r.subtask.as_str())
            .collect::<Vec<_>>(),
        vec!["intro", "method"]
    );
    let facts = facts(&record);
    assert!(facts.contains(&"research:spent=1000000"), "{facts:?}");

    // A budget with half a unit left ends the walk on the budget with no read.
    let tight = decide(&h, request(&pins.0, frontier(), 1_500_000))
        .await
        .unwrap();
    let answer = tight.records.as_slice()[0]
        .answer
        .as_ref()
        .and_then(|a| a.research())
        .unwrap();
    assert_eq!(answer.stop, ResearchStop::Budget);
    assert!(answer.reads.is_empty());

    // A subtask whose budget is gone abstains with its code, naming the subtask.
    let mut exhausted = frontier();
    exhausted.push(budget("b-intro", "intro", 1));
    exhausted.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let record = decide(&h, request(&pins.0, exhausted, 3_000_000))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    let (code, detail) = family_abstention(&record.outcome);
    assert_eq!(code, RESEARCH_NO_BUDGET.as_str());
    assert!(detail.contains("intro"), "{detail}");

    // Cross-tenant negative: another tenant is refused before any read.
    let error = decide_as_other_tenant(&h, request(&pins.0, frontier(), 3_000_000)).await;
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
