//! The `rank` family through the actual Decide handler (MEDULLA-DECISION-ENGINE-R130.3,
//! R130.4, R130.5): options order by the pinned head's calibrated probability, equal
//! codes by the caller's item score; an option without a calibrated probability
//! leaves the ranked list; the sealed record names the ranking premises.

use super::scorer_tests::evaluated;
use super::*;
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use medulla_decision_types::decision::{AbstainReason, PremiseClass};

const IDS: [&str; 3] = ["opt-a", "opt-b", "opt-c"];

fn schema_body() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("signal")]).unwrap(),
    }
}

/// Fit, evaluate and publish a head of `kind` over declared options whose one
/// feature, `signal`, is high only on `opt-a`; returns the pins.
async fn published_head(
    h: &Harness,
    kind: HeadKind,
    id: &str,
) -> (ComponentDependency, ComponentDependency) {
    let body = schema_body();
    let schema_pin = h
        .publish_test_component(
            "schema-rank",
            AgentComponentKind::FeatureSchema,
            "rank features",
            Some(&body),
            None,
        )
        .unwrap();
    let digest = encode_body(&body).unwrap().content_digest;
    let ids: Vec<String> = IDS.iter().map(|s| s.to_string()).collect();
    let mut data = dataset(&digest, &ids, &[1 << 32, 0, 0], 400);
    data.feature_names = BoundedVec::new(vec!["signal".to_string()]).unwrap();
    let gold = super::super::stat_jobs::dataset_digest(&data).unwrap();
    let request = DecisionFitRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: format!("fit-{id}"),
        head_kind: kind,
        feature_schema: schema_pin.clone(),
        policy: DecisionPolicyRef::Default,
        label_regime: LabelRegime::FullLabel {
            gold_set_digest: gold,
        },
        window: window(),
        optimiser: fit_optimiser(),
        source: DatasetSource::Inline {
            dataset: Box::new(data.clone()),
        },
        penalty: None,
        features: Default::default(),
        parent_head: None,
        calibration_method: None,
    };
    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            2,
            &verified(),
            DecisionFitOp::Submit {
                request: Box::new(request),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        draft,
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    let receipt = evaluated(h, 3, (draft_sha256, draft_length), data).await;
    assert!(
        receipt.passed,
        "gates: {:?}",
        receipt.failed_gates.as_slice()
    );
    let head_pin = h
        .publish_test_component(
            id,
            AgentComponentKind::DecisionHead,
            "rank head",
            Some(draft.as_ref()),
            Some(receipt.receipt_digest.clone()),
        )
        .unwrap();
    (schema_pin, head_pin)
}

fn advisory_policy(h: &Harness) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.cold_start = ColdStart::AdvisoryUncalibrated;
    DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-rank-advisory", &policy),
    }
}

/// One declared option: its `signal`, the code it may tie on and its item score.
fn option(id: &str, signal: i64, code: &str, item_score: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![
            DeclaredNumber {
                key: "rank.item_score".to_string(),
                q32: item_score,
            },
            DeclaredNumber {
                key: "signal".to_string(),
                q32: signal,
            },
        ])
        .unwrap(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "rank.code".to_string(),
            text: code.to_string(),
        }])
        .unwrap(),
    }
}

fn rank_request(
    schema: &ComponentDependency,
    head: &ComponentDependency,
    policy: DecisionPolicyRef,
) -> DecideRequest {
    // opt-b and opt-c share one signal and one code; only the item score
    // separates them.
    let options = vec![
        option("opt-a", 1 << 32, "code-a", 0),
        option("opt-b", 0, "code-tied", (1 << 32) / 10),
        option("opt-c", 0, "code-tied", (1 << 32) * 8 / 10),
    ];
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "rank.options".to_string(),
            kind: QuestionKind::Rank,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: Some(head.clone()),
        policy,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn scored_ids(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!("advisory outcome, got {:?}", record.outcome)
    };
    assert!(calibrated, "calibrated probabilities order the list");
    assert!(scores.iter().all(|s| s.probability.is_some()));
    scores.iter().map(|s| s.option_id.clone()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R130.3
#[tokio::test]
async fn decide_orders_by_calibrated_score_and_item_tie_break() {
    let h = Harness::new().await;
    let (schema, head) = published_head(&h, HeadKind::ListwiseLogistic, "head-rank").await;
    let policy = advisory_policy(&h);
    let batch = decide(&h, rank_request(&schema, &head, policy.clone()))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        scored_ids(record),
        ["opt-a", "opt-c", "opt-b"],
        "calibrated score first, then the item score within the shared code"
    );
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Statistical
    );
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    let mut foreign = rank_request(&schema, &head, policy);
    foreign.tenant_id = "tenant-other".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R130.4
#[tokio::test]
async fn uncalibrated_option_abstains_with_its_code() {
    let h = Harness::new().await;
    // A WeightedFeatures head is advisory and carries no calibration, so no
    // option has a calibrated probability.
    let (schema, head) = published_head(&h, HeadKind::WeightedFeatures, "head-rank-weights").await;
    let policy = advisory_policy(&h);
    let batch = decide(&h, rank_request(&schema, &head, policy))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("abstained outcome, got {:?}", record.outcome)
    };
    let mut named: Vec<String> = reasons
        .iter()
        .map(|reason| {
            let AbstainReason::Family {
                family,
                code,
                detail,
            } = reason
            else {
                panic!("a family refusal, got {reason:?}")
            };
            assert_eq!(*family, QuestionKind::Rank);
            assert_eq!(code.as_str(), "UNCALIBRATED_CANDIDATE");
            detail.clone()
        })
        .collect();
    named.sort_unstable();
    assert_eq!(named, IDS, "every uncalibrated option is named once");
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Abstention
    );
}

// spec: MEDULLA-DECISION-ENGINE-R130.5
#[tokio::test]
async fn tied_pair_records_tie_break_premises() {
    let h = Harness::new().await;
    let (schema, head) = published_head(&h, HeadKind::ListwiseLogistic, "head-rank-tied").await;
    let policy = advisory_policy(&h);
    let batch = decide(&h, rank_request(&schema, &head, policy))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let fact = |subject: &str, prefix: &str| -> String {
        record
            .premises
            .iter()
            .find(|p| p.subject == subject && p.fact.starts_with(prefix))
            .unwrap_or_else(|| panic!("premise {prefix} on {subject}"))
            .fact
            .clone()
    };
    for id in IDS {
        let score = record
            .premises
            .iter()
            .find(|p| p.subject == id && p.fact.starts_with("calibrated_score="))
            .unwrap_or_else(|| panic!("calibrated_score premise on {id}"));
        assert_eq!(score.class, PremiseClass::Claim);
        assert!(matches!(
            score.provenance,
            medulla_decision_types::decision::PremiseProvenance::Observation { .. }
        ));
    }
    assert_eq!(
        fact("opt-a", "tie_break_applied="),
        "tie_break_applied=false"
    );
    assert_eq!(
        fact("opt-b", "tie_break_applied="),
        "tie_break_applied=true"
    );
    assert_eq!(
        fact("opt-c", "tie_break_applied="),
        "tie_break_applied=true"
    );
    let policy = record
        .premises
        .iter()
        .find(|p| p.fact == "tie_break_policy=item_score")
        .expect("the tie-break policy is a premise");
    assert_eq!(policy.class, PremiseClass::Definition);
    assert_eq!(
        record.evidence_class,
        EvidenceClass::Claim,
        "the record stays as weak as its weakest premise"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R130.6
#[tokio::test]
async fn tied_codes_resolve_by_item_score_end_to_end() {
    let h = Harness::new().await;
    let (schema, head) = published_head(&h, HeadKind::ListwiseLogistic, "head-rank-e2e").await;
    let policy = advisory_policy(&h);
    let batch = decide(&h, rank_request(&schema, &head, policy.clone()))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    // The served journey publishes a calibrated head, declares tied options and decides:
    // the item score separates the tied pair, the calibrated head ranks the rest.
    assert_eq!(scored_ids(record), ["opt-a", "opt-c", "opt-b"]);
    let applied = |id: &str| -> bool {
        record
            .premises
            .iter()
            .find(|p| p.subject == id && p.fact.starts_with("tie_break_applied="))
            .unwrap_or_else(|| panic!("tie_break_applied premise on {id}"))
            .fact
            == "tie_break_applied=true"
    };
    assert!(!applied("opt-a"));
    assert!(applied("opt-b") && applied("opt-c"));
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "tie_break_policy=item_score"));

    // Swapping the item scores swaps the tied pair and nothing else.
    let mut swapped = rank_request(&schema, &head, policy);
    if let CandidateSource::Declared { options } = &mut swapped.candidates {
        let mut flipped = options.as_slice().to_vec();
        let scores = [0_i64, (1 << 32) * 8 / 10, (1 << 32) / 10];
        for (option, score) in flipped.iter_mut().zip(scores) {
            let mut numbers = option.numbers.as_slice().to_vec();
            for n in &mut numbers {
                if n.key == "rank.item_score" {
                    n.q32 = score;
                }
            }
            option.numbers = BoundedVec::new(numbers).unwrap();
        }
        *options = BoundedVec::new(flipped).unwrap();
    }
    let batch = decide(&h, swapped).await.unwrap();
    assert_eq!(
        scored_ids(&batch.records.as_slice()[0]),
        ["opt-a", "opt-b", "opt-c"]
    );
}
