//! The served `resolve_entity` pair decision (MEDULLA-DECISION-ENGINE-R032.2.2)
//! and the gold-set guarantee its heads are evaluated against
//! (MEDULLA-DECISION-ENGINE-R032.2.3), through the actual Decide and
//! DecisionEval handlers.
//!
//! The acting heads here are built by hand: an inline-fitted head is always
//! synthetic and can never act, so the acting case needs a non-synthetic head
//! with a certified act threshold. It is published through the real publish
//! gate, with a passed receipt written for exactly its bytes.

use super::*;
use medulla_decision_stats::decision::evaluate::seal_receipt;
use medulla_decision_types::decision::jobs::{DecisionEvalReceipt, GoldGuarantee, LabelExclusions};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use medulla_decision_types::decision::statistical::head::{
    DecisionHeadBody, FeatureStandardisation, FittedRegime, HeadCalibration,
    DECISION_HEAD_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::{RiskMethod, RiskStatement};
use medulla_decision_types::decision::{AbstainReason, PremiseClass, PremiseProvenance};
use medulla_persistence_records::decision_jobs::{encode_artifact, receipt_key};

const ONE: i64 = 1 << 32;

fn q(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

fn entity(name: &str) -> String {
    format!("https://example.test/entity/{name}")
}

fn schema_body() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("signal")]).unwrap(),
    }
}

/// A one-feature linear head over `signal` whose prediction set holds an
/// option when its nonconformity `1 - p` is at most `set_threshold`.
fn acting_head_body(schema_digest: &str, set_threshold: f64) -> DecisionHeadBody {
    DecisionHeadBody {
        schema_version: DECISION_HEAD_SCHEMA_VERSION,
        kind: HeadKind::ListwiseLogistic,
        regime: FittedRegime::FullLabel,
        feature_schema_digest: schema_digest.to_string(),
        standardisation: BoundedVec::new(vec![FeatureStandardisation {
            center: q(0),
            scale: q(ONE),
            lower: q(-8 * ONE),
            upper: q(8 * ONE),
        }])
        .unwrap(),
        weights: BoundedVec::new(vec![q(ONE)]).unwrap(),
        calibration: Some(HeadCalibration {
            inverse_temperature: q(ONE),
            alpha: rational(1, 10),
            set_threshold: q((set_threshold * ONE as f64) as i64),
            coverage_lower: rational(9, 10),
            coverage_upper: rational(1, 1),
            act_threshold: Some(q(ONE * 6 / 10)),
            risk: Some(RiskStatement {
                method: RiskMethod::LearnThenTest,
                epsilon: rational(1, 20),
                delta: rational(1, 20),
                n_calibration: 200,
            }),
            n_calibration: 200,
            synthetic: false,
            isotonic: None,
        }),
        training_records_digest: format!("sha256:{}", "ab".repeat(32)),
        n_training: 200,
        synthetic: false,
        scorer: None,
    }
}

/// Publish the pair feature schema once.
fn publish_schema(h: &Harness) -> (ComponentDependency, String) {
    let schema = schema_body();
    let pin = h
        .publish_test_component(
            "schema-entity",
            AgentComponentKind::FeatureSchema,
            "entity pair features",
            Some(&schema),
            None,
        )
        .unwrap();
    (pin, encode_body(&schema).unwrap().content_digest)
}

/// Publish an acting head with the given prediction-set threshold, behind a
/// passed receipt written for exactly the head's bytes.
fn publish_acting_head(
    h: &Harness,
    schema_digest: &str,
    id: &str,
    set_threshold: f64,
) -> ComponentDependency {
    let head = acting_head_body(schema_digest, set_threshold);
    let head_digest = encode_body(&head).unwrap().content_digest;
    let mut receipt = DecisionEvalReceipt {
        receipt_digest: String::new(),
        head_digest,
        policy_digest: medulla_decision_types::decision::digest::policy_digest(
            &medulla_decision_types::decision::DecisionPolicy::engine_default(),
        ),
        n_records: 200,
        estimates: BoundedVec::default(),
        calibration: None,
        metrics: None,
        promotion: None,
        exclusions: LabelExclusions::default(),
        pooled: BoundedVec::default(),
        failed_gates: BoundedVec::default(),
        feature_power: BoundedVec::default(),
        premises: BoundedVec::default(),
        passed: true,
        synthetic: false,
        gold_guarantee: None,
    };
    seal_receipt(&mut receipt);
    h.store
        .put_decision_artifacts(
            TENANT,
            &[(
                receipt_key(&receipt.receipt_digest),
                encode_artifact(&receipt).unwrap(),
            )],
        )
        .unwrap();
    h.publish_test_component(
        id,
        AgentComponentKind::DecisionHead,
        "entity pair head",
        Some(&head),
        Some(receipt.receipt_digest.clone()),
    )
    .unwrap()
}

/// One pair option: its two entity IRIs and the feature `signal`.
fn pair_option(id: &str, left: &str, right: &str, signal: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "signal".to_string(),
            q32: signal,
        }])
        .unwrap(),
        texts: BoundedVec::new(vec![
            DeclaredText {
                key: "entity.left".to_string(),
                text: left.to_string(),
            },
            DeclaredText {
                key: "entity.right".to_string(),
                text: right.to_string(),
            },
        ])
        .unwrap(),
    }
}

/// The two pairs every decision here weighs: `pair-a` scores high, `pair-b` low.
fn two_pairs() -> Vec<DeclaredOption> {
    vec![
        pair_option("pair-a", &entity("a-left"), &entity("a-right"), 3 * ONE),
        pair_option("pair-b", &entity("b-left"), &entity("b-right"), 0),
    ]
}

fn pair_request(
    schema: &ComponentDependency,
    head: Option<&ComponentDependency>,
    policy: DecisionPolicyRef,
    options: Vec<DeclaredOption>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "entity.pair".to_string(),
            kind: QuestionKind::ResolveEntity,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: head.cloned(),
        policy,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn policy_with(h: &Harness, id: &str, knobs: &[(&str, KnobValue)]) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    for (name, value) in knobs {
        policy.knobs.insert(KnobName::new(name), *value);
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(id, &policy),
    }
}

fn abstains_insufficient(record: &StatisticalDecisionRecord) {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome)
    };
    assert_eq!(reasons.as_slice(), [AbstainReason::InsufficientConfidence]);
}

// spec: MEDULLA-DECISION-ENGINE-R032.2.2
#[tokio::test]
async fn pairs_act_only_inside_the_conformal_set() {
    let h = Harness::new().await;

    // A head whose prediction set is exactly the top pair acts on it.
    let (schema, digest) = publish_schema(&h);
    let singleton = publish_acting_head(&h, &digest, "head-entity-singleton", 0.1);
    let batch = decide(
        &h,
        pair_request(
            &schema,
            Some(&singleton),
            DecisionPolicyRef::Default,
            two_pairs(),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::ResolveEntity);
    let StatisticalOutcome::Acted {
        option_id,
        prediction_set,
        ..
    } = &record.outcome
    else {
        panic!("a singleton set acts: {:?}", record.outcome)
    };
    assert_eq!(option_id, "pair-a");
    assert_eq!(prediction_set.as_slice(), ["pair-a"]);

    // The record names each pair and the head's calibration; it proposes a
    // link and writes nothing.
    let pair_facts: Vec<&str> = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("entity_pair:"))
        .map(|p| p.fact.as_str())
        .collect();
    assert_eq!(
        pair_facts,
        [
            format!("entity_pair:{}={}", entity("a-left"), entity("a-right")),
            format!("entity_pair:{}={}", entity("b-left"), entity("b-right")),
        ]
    );
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("entity_pair:"))
        .all(|p| p.class == PremiseClass::Claim));
    let calibration = record
        .premises
        .iter()
        .find(|p| p.subject == "head" && p.fact == "calibration")
        .expect("the head's calibration is a premise");
    assert_eq!(calibration.class, PremiseClass::Observation);
    assert!(matches!(
        calibration.provenance,
        PremiseProvenance::Observation { .. }
    ));

    // A head whose prediction set holds both pairs is not sure which is the
    // match: the decision abstains instead of acting on the top pair.
    let wide = publish_acting_head(&h, &digest, "head-entity-wide", 0.96);
    let batch = decide(
        &h,
        pair_request(
            &schema,
            Some(&wide),
            DecisionPolicyRef::Default,
            two_pairs(),
        ),
    )
    .await
    .unwrap();
    abstains_insufficient(&batch.records.as_slice()[0]);

    // The miscoverage bound is the pinned policy's knob: a head calibrated at
    // 0.1 is looser than a policy that allows 0.05, so it abstains.
    let tight = policy_with(
        &h,
        "policy-entity-alpha",
        &[("resolve_entity.alpha", KnobValue::DecimalMicros(50_000))],
    );
    let batch = decide(
        &h,
        pair_request(&schema, Some(&singleton), tight, two_pairs()),
    )
    .await
    .unwrap();
    abstains_insufficient(&batch.records.as_slice()[0]);
    let knob_premise = batch.records.as_slice()[0]
        .premises
        .iter()
        .find(|p| p.fact.starts_with("knob:resolve_entity.alpha="))
        .expect("the alpha the decision read is a sealed premise");
    assert!(matches!(
        knob_premise.provenance,
        PremiseProvenance::Policy { .. }
    ));

    // A policy that allows 0.2 admits the same head.
    let loose = policy_with(
        &h,
        "policy-entity-alpha-loose",
        &[("resolve_entity.alpha", KnobValue::DecimalMicros(200_000))],
    );
    let batch = decide(
        &h,
        pair_request(&schema, Some(&singleton), loose, two_pairs()),
    )
    .await
    .unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Acted { .. }
    ));

    // Without a head nothing is decided: the default policy abstains.
    let batch = decide(
        &h,
        pair_request(&schema, None, DecisionPolicyRef::Default, two_pairs()),
    )
    .await
    .unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // Both refusals are typed.
    let mut missing_right = two_pairs();
    missing_right[0].texts =
        BoundedVec::new(vec![missing_right[0].texts.as_slice()[0].clone()]).unwrap();
    let refused = decide(
        &h,
        pair_request(
            &schema,
            Some(&singleton),
            DecisionPolicyRef::Default,
            missing_right,
        ),
    )
    .await
    .unwrap_err();
    assert!(
        refused.starts_with("ENTITY_PAIR_INVALID"),
        "expected ENTITY_PAIR_INVALID, got {refused}"
    );
    let not_an_iri = vec![pair_option("pair-a", "not an iri", &entity("a-right"), ONE)];
    let refused = decide(
        &h,
        pair_request(
            &schema,
            Some(&singleton),
            DecisionPolicyRef::Default,
            not_an_iri,
        ),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("ENTITY_PAIR_INVALID"), "got {refused}");
    let itself = vec![pair_option("pair-a", &entity("same"), &entity("same"), ONE)];
    let refused = decide(
        &h,
        pair_request(
            &schema,
            Some(&singleton),
            DecisionPolicyRef::Default,
            itself,
        ),
    )
    .await
    .unwrap_err();
    assert!(
        refused.starts_with("ENTITY_PAIR_SELF"),
        "expected ENTITY_PAIR_SELF, got {refused}"
    );

    // Another tenant's caller is refused.
    let mut foreign = pair_request(
        &schema,
        Some(&singleton),
        DecisionPolicyRef::Default,
        two_pairs(),
    );
    foreign.tenant_id = "tenant-other".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

/// A gold set of `n` pairs where the head's prediction set `{pair-a}` holds
/// the acceptable option for exactly `covered` of them.
fn gold_set(schema_digest: &str, n: usize, covered: usize) -> LabelledDataset {
    let ids = vec!["pair-a".to_string(), "pair-b".to_string()];
    let items = (0..n)
        .map(|i| LabelledItem {
            item_id: format!("gold-{i:04}"),
            recorded_at_ms: 1_000,
            class_key: "entity".to_string(),
            candidate_ids: BoundedVec::new(ids.clone()).unwrap(),
            features: BoundedVec::new(vec![3 * ONE, 0]).unwrap(),
            label: ItemLabel::Gold {
                acceptable: BoundedVec::new(vec![if i < covered {
                    "pair-a".to_string()
                } else {
                    "pair-b".to_string()
                }])
                .unwrap(),
                source: LabelSource::SyntheticConstruction,
            },
            audit_inclusion: None,
        })
        .collect();
    LabelledDataset {
        schema_version: LABELLED_DATASET_SCHEMA_VERSION,
        feature_schema_digest: schema_digest.to_string(),
        feature_names: BoundedVec::new(vec!["signal".to_string()]).unwrap(),
        scale: QuantScaleTag::Q32,
        items: BoundedVec::new(items).unwrap(),
        synthetic: true,
    }
}

async fn evaluate_gold(
    h: &Harness,
    request_id: u64,
    head: &ComponentDependency,
    data: LabelledDataset,
    policy: DecisionPolicyRef,
    guarantee: Option<GoldGuarantee>,
) -> Result<DecisionEvalReceipt, String> {
    let gold = super::super::stat_jobs::dataset_digest(&data).unwrap();
    let eval = DecisionEvalRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: format!("eval-gold-{request_id}"),
        candidate: EvalCandidate::PublishedHead { head: head.clone() },
        policy,
        estimators: BoundedVec::default(),
        gold_set_digest: Some(gold),
        window: window(),
        source: DatasetSource::Inline {
            dataset: Box::new(data),
        },
        mode: medulla_decision_types::decision::EvalMode::OffPolicy,
        gold_guarantee: guarantee,
    };
    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_eval(
            &h.state,
            request_id,
            &verified(),
            DecisionEvalOp::Submit {
                request: Box::new(eval),
            },
        )
        .await,
    )?;
    let DecisionJobOutput::Eval { receipt } = succeeded(&job).clone() else {
        panic!("eval output")
    };
    Ok(*receipt)
}

fn gate_named(receipt: &DecisionEvalReceipt, code: &str) -> bool {
    receipt.failed_gates.iter().any(|gate| gate == code)
}

// spec: MEDULLA-DECISION-ENGINE-R032.2.3
#[tokio::test]
async fn gold_set_guarantee_holds_or_the_decision_abstains() {
    let h = Harness::new().await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-entity-gold", 0.1);
    let ask = Some(GoldGuarantee::ResolveEntity);

    // Every gold pair covered: the guarantee holds, and the receipt records
    // the alpha it was held to, the counts and the gold set's digest.
    let full = gold_set(&digest, 200, 200);
    let gold_digest = super::super::stat_jobs::dataset_digest(&full).unwrap();
    let held = evaluate_gold(&h, 10, &head, full, DecisionPolicyRef::Default, ask)
        .await
        .unwrap();
    assert!(!gate_named(&held, "CONFORMAL_COVERAGE_UNMET"));
    assert!(!gate_named(&held, "GOLD_SET_TOO_SMALL"));
    let evidence = held
        .gold_guarantee
        .as_ref()
        .expect("the evidence is recorded");
    assert_eq!(evidence.guarantee, GoldGuarantee::ResolveEntity);
    assert_eq!(evidence.n_gold, 200);
    assert_eq!(evidence.covered, 200);
    assert_eq!(evidence.min_pairs, 200);
    assert_eq!(
        evidence.alpha.numerator() * 10,
        evidence.alpha.denominator()
    );
    assert_eq!(evidence.gold_set_digest, gold_digest);

    // Coverage under 1 - alpha: 150 of 200 is short of 180. The receipt fails
    // by name and the head cannot be promoted on it.
    let short = evaluate_gold(
        &h,
        11,
        &head,
        gold_set(&digest, 200, 150),
        DecisionPolicyRef::Default,
        ask,
    )
    .await
    .unwrap();
    assert!(gate_named(&short, "CONFORMAL_COVERAGE_UNMET"));
    assert!(!gate_named(&short, "GOLD_SET_TOO_SMALL"));
    assert!(!short.passed);
    let refused = h.publish_test_component(
        "head-entity-gold",
        AgentComponentKind::DecisionHead,
        "entity pair head",
        Some(&acting_head_body(&digest, 0.1)),
        Some(short.receipt_digest.clone()),
    );
    let refused = refused.unwrap_err();
    assert!(
        refused.starts_with("EVALUATION_RECEIPT_MISMATCH"),
        "{refused}"
    );
    assert!(refused.contains("CONFORMAL_COVERAGE_UNMET"), "{refused}");

    // Fewer pairs than the floor: the coverage is not believed, whatever it is.
    let small = evaluate_gold(
        &h,
        12,
        &head,
        gold_set(&digest, 60, 60),
        DecisionPolicyRef::Default,
        ask,
    )
    .await
    .unwrap();
    assert!(gate_named(&small, "GOLD_SET_TOO_SMALL"));
    assert!(!gate_named(&small, "CONFORMAL_COVERAGE_UNMET"));
    assert!(!small.passed);

    // The floor is the pinned policy's knob.
    let low_floor = policy_with(
        &h,
        "policy-entity-floor",
        &[("resolve_entity.gold_min_pairs", KnobValue::Integer(50))],
    );
    let accepted = evaluate_gold(&h, 13, &head, gold_set(&digest, 60, 60), low_floor, ask)
        .await
        .unwrap();
    assert!(!gate_named(&accepted, "GOLD_SET_TOO_SMALL"));
    assert_eq!(accepted.gold_guarantee.as_ref().unwrap().min_pairs, 50);

    // So is alpha: 185 of 200 reaches 1 - 0.1 but not 1 - 0.05.
    let reaching = gold_set(&digest, 200, 185);
    let at_default = evaluate_gold(
        &h,
        14,
        &head,
        reaching.clone(),
        DecisionPolicyRef::Default,
        ask,
    )
    .await
    .unwrap();
    assert!(!gate_named(&at_default, "CONFORMAL_COVERAGE_UNMET"));
    let tight = policy_with(
        &h,
        "policy-entity-gold-alpha",
        &[("resolve_entity.alpha", KnobValue::DecimalMicros(50_000))],
    );
    let at_tight = evaluate_gold(&h, 15, &head, reaching, tight.clone(), ask)
        .await
        .unwrap();
    assert!(gate_named(&at_tight, "CONFORMAL_COVERAGE_UNMET"));

    // Where the guarantee is unmet at the policy's alpha, the decision at that
    // alpha abstains rather than acting on the head.
    let batch = decide(&h, pair_request(&schema, Some(&head), tight, two_pairs()))
        .await
        .unwrap();
    abstains_insufficient(&batch.records.as_slice()[0]);

    // No guarantee asked: nothing is recorded and neither code is raised.
    let plain = evaluate_gold(
        &h,
        16,
        &head,
        gold_set(&digest, 60, 0),
        DecisionPolicyRef::Default,
        None,
    )
    .await
    .unwrap();
    assert!(plain.gold_guarantee.is_none());
    assert!(!gate_named(&plain, "GOLD_SET_TOO_SMALL"));
    assert!(!gate_named(&plain, "CONFORMAL_COVERAGE_UNMET"));

    // Another tenant cannot read or evaluate this tenant's head.
    let mut foreign = DecisionEvalRequest {
        tenant_id: "foreign".to_string(),
        idempotency_key: "eval-gold-foreign".to_string(),
        candidate: EvalCandidate::PublishedHead { head: head.clone() },
        policy: DecisionPolicyRef::Default,
        estimators: BoundedVec::default(),
        gold_set_digest: None,
        window: window(),
        source: DatasetSource::Inline {
            dataset: Box::new(gold_set(&digest, 60, 60)),
        },
        mode: medulla_decision_types::decision::EvalMode::OffPolicy,
        gold_guarantee: ask,
    };
    foreign.gold_set_digest =
        Some(super::super::stat_jobs::dataset_digest(&gold_set(&digest, 60, 60)).unwrap());
    let denied = decode::<DecisionJobRecord>(
        super::super::jobs::handle_decision_eval(
            &h.state,
            17,
            &verified(),
            DecisionEvalOp::Submit {
                request: Box::new(foreign),
            },
        )
        .await,
    );
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));
}
