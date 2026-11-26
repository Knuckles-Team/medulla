//! MEDULLA-DECISION-ENGINE-R134.12 and R134.12.1: a served `TrajectoryLadder`
//! question picks one legal rung of the caller's ladder; paired verdicts are
//! stored through `DecisionLog` (`RunRecord::PairedVerdict`) and `Decide`
//! writes nothing.

use super::*;
use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::program_store::{RunRecord, RunRecordStored, StoredRun};
use medulla_decision_types::decision::statistical::head::{
    DecisionHeadBody, FeatureStandardisation, FittedRegime, HeadKind, DECISION_HEAD_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::log::{
    DecisionLogEntry, EntryInputs, RecordVisibility, DECISION_LOG_SCHEMA_VERSION,
};
use medulla_decision_types::decision::trajectory_ladder::{
    PairedVerdict, RungChoice, TrajectoryLadderRequest, TrajectoryRung, Winner,
};
use medulla_decision_types::decision::ResolutionKind;
use medulla_decision_types::decision::{DecisionPolicy, QuantScaleTag, QuantisedValue};
use medulla_persistence_records::decision_jobs::{encode_artifact, record_key};
use medulla_types::contract::BoundedVec;

const FOREIGN: &str = "tenant-foreign";
const SNAPSHOT: &str = "snapshot-digest-1";
const ARM: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const BASE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn rung(id: &str, width: u32, depth: u32) -> TrajectoryRung {
    TrajectoryRung {
        rung_id: id.into(),
        width,
        depth,
    }
}

fn ladder(max_width: u32, rungs: Vec<TrajectoryRung>) -> TrajectoryLadderRequest {
    TrajectoryLadderRequest {
        task_digest: "task-1".into(),
        input_snapshot_digest: SNAPSHOT.into(),
        max_width,
        rungs,
        improvement_claim: None,
    }
}

fn standard() -> Vec<TrajectoryRung> {
    vec![
        rung("w4d2", 4, 2),
        rung("w1d1", 1, 1),
        rung("w2d1", 2, 1),
        rung("w2d2", 2, 2),
        rung("w1d9", 1, 9),
        rung("w8d1", 8, 1),
    ]
}

fn ask_in(
    h: &Harness,
    tenant: &str,
    asked: &TrajectoryLadderRequest,
) -> Result<StatisticalDecisionRecord, String> {
    ask_with(h, tenant, asked, DecisionPolicyRef::Default, None)
}

fn ask_with(
    h: &Harness,
    tenant: &str,
    asked: &TrajectoryLadderRequest,
    policy: DecisionPolicyRef,
    head: Option<ComponentDependency>,
) -> Result<StatisticalDecisionRecord, String> {
    let schema = publish_route_schema_in(h, tenant);
    let mut request = declared_request(
        &schema,
        tenant,
        QuestionKind::TrajectoryLadder,
        "ladder",
        policy,
        vec![TypedParam {
            name: "ladder".to_string(),
            value: TypedValue::Text(serde_json::to_string(asked).unwrap()),
        }],
    );
    request.head = head;
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
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
        &request,
        None,
        0,
        "decider".to_string(),
        std::time::Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn choice_of(record: &StatisticalDecisionRecord) -> RungChoice {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let id = &scores.as_slice()[0].option_id;
            serde_json::from_str(id.strip_prefix("choice:").expect("a choice answer")).unwrap()
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn rows(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, "run_record/", 100)
        .unwrap()
        .len()
}

/// A logged ladder decision of `tenant`, written as the log commit writes it.
fn log_decision(h: &Harness, tenant: &str, asked: &TrajectoryLadderRequest) -> String {
    let record = ask_in(h, tenant, asked).unwrap();
    let entry = DecisionLogEntry {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        record: Box::new(record.clone()),
        committed_by: "decider".to_string(),
        committed_at_ms: 1_000,
        visibility: RecordVisibility::Tenant,
        inputs: EntryInputs::Inline,
        approval: None,
    };
    h.store
        .put_decision_artifacts(
            tenant,
            &[(
                record_key(&record.record_id),
                encode_artifact(&entry).unwrap(),
            )],
        )
        .unwrap();
    record.record_id
}

/// A head that scores only the width of a rung, higher width higher.
fn width_head(h: &Harness) -> ComponentDependency {
    let q = |value: i64| QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    };
    let stand = || FeatureStandardisation {
        center: q(0),
        scale: q(1 << 32),
        lower: q(-(1 << 40)),
        upper: q(1 << 40),
    };
    let body = DecisionHeadBody {
        schema_version: DECISION_HEAD_SCHEMA_VERSION,
        kind: HeadKind::WeightedFeatures,
        regime: FittedRegime::FullLabel,
        feature_schema_digest: "sha256:ladder".to_string(),
        standardisation: BoundedVec::new(vec![stand(), stand()]).unwrap(),
        weights: BoundedVec::new(vec![q(1 << 32), q(0)]).unwrap(),
        calibration: None,
        training_records_digest: "sha256:verdicts".to_string(),
        n_training: 1,
        synthetic: false,
        scorer: None,
    };
    h.publish_test_component(
        "head-ladder",
        AgentComponentKind::DecisionHead,
        "ladder head",
        Some(&body),
        None,
    )
    .unwrap()
}

async fn write_verdict(h: &Harness, verdict: PairedVerdict) -> Result<RunRecordStored, String> {
    let op = DecisionLogOp::RunRecord {
        tenant_id: TENANT.to_string(),
        record: RunRecord::PairedVerdict { verdict },
    };
    decode(super::log_tests::log_op(h, "ladderer", op).await)
}

fn verdict(decision_id: &str, rung_id: &str, winner: Winner) -> PairedVerdict {
    PairedVerdict {
        decision_id: decision_id.into(),
        rung_id: rung_id.into(),
        baseline_id: "w1d1".into(),
        input_snapshot_digest: SNAPSHOT.into(),
        arm_outcome_digest: ARM.into(),
        baseline_outcome_digest: BASE.into(),
        winner,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134.12
#[tokio::test]
async fn ladder_picks_smallest_rung_without_head_and_head_rung_with_one() {
    let h = Harness::new().await;
    // Legal under max_width 4 and max_depth 3: w1d1, w2d1, w2d2, w4d2 (w1d9 is too deep,
    // w8d1 too wide). The order is width * depth, then width, then depth, then id.
    let record = ask_in(&h, TENANT, &ladder(4, standard())).unwrap();
    let choice = choice_of(&record);
    assert_eq!(choice.rung_id, "w1d1", "index 0 is the smallest legal rung");
    assert_eq!(choice.baseline_id, "w1d1");
    assert!(!choice.ranked_by_head);
    assert_eq!(choice.trajectory_ids.len(), 1);
    assert!(choice.trajectory_ids[0].ends_with("-t1"));
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(rows(&h, TENANT), 0, "Decide writes nothing");
    // The same request chooses the same rung and ids.
    assert_eq!(
        choice_of(&ask_in(&h, TENANT, &ladder(4, standard())).unwrap()),
        choice
    );

    // The knob index past the last legal rung picks the last one.
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("trajectory.default_rung"),
        KnobValue::Integer(15),
    );
    let pin = h.publish_policy("policy-ladder-rung", &policy);
    let picked = choice_of(
        &ask_with(
            &h,
            TENANT,
            &ladder(4, standard()),
            DecisionPolicyRef::Pinned { component: pin },
            None,
        )
        .unwrap(),
    );
    assert_eq!(picked.rung_id, "w4d2");
    assert_eq!(picked.trajectory_ids.len(), 4);
    assert!(picked.trajectory_ids[3].ends_with("-t4"));

    // A promoted head ranks the legal rungs only: it prefers width, so it picks w4d2
    // and never the wider, illegal w8d1.
    let head = width_head(&h);
    let ranked = choice_of(
        &ask_with(
            &h,
            TENANT,
            &ladder(4, standard()),
            DecisionPolicyRef::Default,
            Some(head.clone()),
        )
        .unwrap(),
    );
    assert_eq!(ranked.rung_id, "w4d2");
    assert!(ranked.ranked_by_head);

    // Each refusal is typed.
    let no_baseline = ladder(4, vec![rung("w2d1", 2, 1)]);
    assert!(ask_in(&h, TENANT, &no_baseline)
        .unwrap_err()
        .starts_with("TRAJECTORY_BASELINE_MISSING"));
    let none_legal = ladder(0, standard());
    assert!(ask_in(&h, TENANT, &none_legal)
        .unwrap_err()
        .starts_with("TRAJECTORY_NO_LEGAL_RUNG"));

    // A head of another tenant is never read: the pin names no component there.
    let foreign = ask_with(
        &h,
        FOREIGN,
        &ladder(4, standard()),
        DecisionPolicyRef::Default,
        Some(head),
    );
    assert!(foreign.is_err(), "{foreign:?}");
    assert_eq!(rows(&h, TENANT), 0);
}

// spec: MEDULLA-DECISION-ENGINE-R134.12.1
#[tokio::test]
async fn paired_verdict_requires_complete_arms() {
    let h = Harness::new().await;
    let asked = ladder(4, standard());
    let decision = log_decision(&h, TENANT, &asked);

    // A complete verdict is stored and reads back.
    let stored = write_verdict(&h, verdict(&decision, "w2d1", Winner::Arm))
        .await
        .unwrap();
    assert!(!stored.replayed);
    assert!(matches!(stored.stored, StoredRun::PairedVerdict { .. }));
    let held = h
        .store
        .decision_artifact(TENANT, &stored.key)
        .unwrap()
        .expect("the verdict row");
    let back: PairedVerdict =
        medulla_persistence_records::decision_jobs::decode_artifact(&held, "paired verdict")
            .unwrap();
    assert_eq!(back, verdict(&decision, "w2d1", Winner::Arm));
    let again = write_verdict(&h, verdict(&decision, "w2d1", Winner::Arm))
        .await
        .unwrap();
    assert!(again.replayed);
    let stored_rows = rows(&h, TENANT);

    // Each refusal is typed and stores nothing.
    let mut no_arm = verdict(&decision, "w2d2", Winner::Arm);
    no_arm.arm_outcome_digest = String::new();
    assert!(write_verdict(&h, no_arm)
        .await
        .unwrap_err()
        .starts_with("TRAJECTORY_ARM_MISSING"));
    let mut other_snapshot = verdict(&decision, "w2d2", Winner::Arm);
    other_snapshot.input_snapshot_digest = "another".into();
    assert!(write_verdict(&h, other_snapshot)
        .await
        .unwrap_err()
        .starts_with("TRAJECTORY_SNAPSHOT_MISMATCH"));
    assert!(
        write_verdict(&h, verdict("no-such-decision", "w2d2", Winner::Arm))
            .await
            .unwrap_err()
            .starts_with("TRAJECTORY_DECISION_UNKNOWN")
    );
    assert_eq!(rows(&h, TENANT), stored_rows);

    // A decision of another tenant is unknown here, never confirmed.
    let foreign = log_decision(&h, FOREIGN, &asked);
    assert!(write_verdict(&h, verdict(&foreign, "w2d1", Winner::Arm))
        .await
        .unwrap_err()
        .starts_with("TRAJECTORY_DECISION_UNKNOWN"));

    // An improvement claim needs a stored verdict whose winner is the arm.
    let mut claim = ladder(4, standard());
    claim.improvement_claim = Some("w2d1".into());
    assert!(ask_in(&h, TENANT, &claim).is_ok());
    claim.improvement_claim = Some("w2d2".into());
    assert!(ask_in(&h, TENANT, &claim)
        .unwrap_err()
        .starts_with("TRAJECTORY_IMPROVEMENT_UNPROVEN"));
    write_verdict(&h, verdict(&decision, "w4d2", Winner::Baseline))
        .await
        .unwrap();
    claim.improvement_claim = Some("w4d2".into());
    assert!(ask_in(&h, TENANT, &claim)
        .unwrap_err()
        .starts_with("TRAJECTORY_IMPROVEMENT_UNPROVEN"));
    // Decide wrote nothing beyond the two verdict rows.
    assert_eq!(rows(&h, TENANT), stored_rows + 1);
}

/// The ladder head's feature schema: exactly `width` and `depth`.
fn ladder_schema(h: &Harness) -> (ComponentDependency, String) {
    use medulla_decision_types::decision::statistical::body::encode_body;
    use medulla_decision_types::decision::statistical::features::{
        FeatureSchemaBody, FEATURE_SCHEMA_VERSION,
    };
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("width"), number_feature("depth")]).unwrap(),
    };
    let pin = h
        .publish_test_component(
            "schema-ladder-head",
            AgentComponentKind::FeatureSchema,
            "ladder head features",
            Some(&body),
            None,
        )
        .unwrap();
    (pin, encode_body(&body).unwrap().content_digest)
}

fn verdict_fit(
    pin: &ComponentDependency,
    key: &str,
    gold_set_digest: String,
) -> medulla_decision_types::decision::DecisionFitRequest {
    use medulla_decision_types::decision::jobs::{DatasetSource, LabelRegime};
    medulla_decision_types::decision::DecisionFitRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: key.to_string(),
        head_kind: HeadKind::ListwiseLogistic,
        feature_schema: pin.clone(),
        policy: DecisionPolicyRef::Default,
        label_regime: LabelRegime::FullLabel { gold_set_digest },
        window: window(),
        optimiser: fit_optimiser(),
        source: DatasetSource::PairedVerdicts,
        calibration_method: None,
        penalty: None,
        features: Default::default(),
        parent_head: None,
    }
}

async fn submit(
    h: &Harness,
    request: medulla_decision_types::decision::DecisionFitRequest,
) -> medulla_protocol_types::protocol::Response {
    super::super::jobs::handle_decision_fit(
        &h.state,
        7,
        &verified(),
        medulla_decision_types::decision::DecisionFitOp::Submit {
            request: Box::new(request),
        },
    )
    .await
}

// spec: MEDULLA-DECISION-ENGINE-R134.12.1
#[tokio::test]
async fn fit_reads_stored_verdicts_as_the_ladder_dataset() {
    use medulla_decision_types::decision::DecisionJobRecord;
    let h = Harness::new().await;
    let (pin, schema_digest) = ladder_schema(&h);

    // With no stored verdict there is nothing to fit.
    let none = super::super::stat_verdict_dataset::verdict_dataset(
        h.store.as_ref(),
        TENANT,
        &schema_digest,
    );
    assert!(none.unwrap_err().contains("NO_ADMISSIBLE_LABELS"));

    // Eight ladder decisions, each with three verdicts: the wider rung wins
    // on w2d1, loses on w2d2 and ties on w4d2.
    for n in 0..8 {
        let mut asked = ladder(4, standard());
        asked.task_digest = format!("task-{n}");
        let decision = log_decision(&h, TENANT, &asked);
        for (rung, winner) in [
            ("w2d1", Winner::Arm),
            ("w2d2", Winner::Baseline),
            ("w4d2", Winner::Tie),
        ] {
            write_verdict(&h, verdict(&decision, rung, winner))
                .await
                .unwrap();
        }
    }
    // A verdict of another tenant's decision is never in this tenant's set.
    let foreign = log_decision(&h, FOREIGN, &ladder(4, standard()));
    assert!(write_verdict(&h, verdict(&foreign, "w2d1", Winner::Arm))
        .await
        .is_err());

    let dataset = super::super::stat_verdict_dataset::verdict_dataset(
        h.store.as_ref(),
        TENANT,
        &schema_digest,
    )
    .unwrap();
    assert_eq!(dataset.items.len(), 24, "one item per stored verdict");
    assert_eq!(
        dataset.feature_names.as_slice(),
        ["width".to_string(), "depth".to_string()]
    );
    assert!(dataset.synthetic, "stored verdicts are caller-authored");
    let tie = dataset
        .items
        .iter()
        .find(|item| item.item_id.ends_with("/w4d2"))
        .unwrap();
    assert_eq!(tie.candidate_ids.as_slice(), ["w4d2", "w1d1"]);
    assert_eq!(
        tie.features.as_slice(),
        [4 << 32, 2 << 32, 1 << 32, 1 << 32]
    );
    let medulla_decision_types::decision::statistical::dataset::ItemLabel::Gold {
        acceptable, ..
    } = &tie.label
    else {
        panic!("a verdict is a gold label")
    };
    assert_eq!(
        acceptable.as_slice(),
        ["w4d2", "w1d1"],
        "a tie accepts both"
    );

    // A fit pinned by the stored verdicts' digest trains on exactly them.
    let digest = super::super::stat_jobs::dataset_digest(&dataset).unwrap();
    let job: DecisionJobRecord =
        decode(submit(&h, verdict_fit(&pin, "fit-verdicts", digest.clone())).await).unwrap();
    let medulla_decision_types::decision::DecisionJobOutput::Fit { draft, .. } = succeeded(&job)
    else {
        panic!("fit output")
    };
    // A full-label listwise fit holds out every stride-th item for calibration.
    let held_out = |items: usize| items / medulla_decision_stats::decision::fit::CALIBRATION_STRIDE;
    assert_eq!(draft.n_training as usize, 24 - held_out(24));
    assert_eq!(draft.weights.len(), 2, "one weight per ladder feature");
    assert!(draft.synthetic);
    let first_lineage = draft.training_records_digest.clone();

    // A wrong pin is refused and names the digest the stored verdicts hash to.
    let refused = submit(
        &h,
        verdict_fit(&pin, "fit-wrong-pin", "sha256:wrong".into()),
    )
    .await;
    let error = decode::<DecisionJobRecord>(refused)
        .map(|record| format!("{:?}", record.outcome))
        .unwrap_or_else(|error| error);
    assert!(
        error.contains(&format!("the stored verdicts hash to {digest}")),
        "{error}"
    );

    // A new verdict changes the dataset, so the head's lineage changes with it.
    let mut asked = ladder(4, standard());
    asked.task_digest = "task-extra".into();
    let decision = log_decision(&h, TENANT, &asked);
    write_verdict(&h, verdict(&decision, "w2d1", Winner::Arm))
        .await
        .unwrap();
    let grown = super::super::stat_verdict_dataset::verdict_dataset(
        h.store.as_ref(),
        TENANT,
        &schema_digest,
    )
    .unwrap();
    assert_eq!(grown.items.len(), 25);
    let digest = super::super::stat_jobs::dataset_digest(&grown).unwrap();
    let job: DecisionJobRecord =
        decode(submit(&h, verdict_fit(&pin, "fit-verdicts-grown", digest)).await).unwrap();
    let medulla_decision_types::decision::DecisionJobOutput::Fit { draft, .. } = succeeded(&job)
    else {
        panic!("fit output")
    };
    assert_eq!(draft.n_training as usize, 25 - held_out(25));
    assert_ne!(draft.training_records_digest, first_lineage);
}
