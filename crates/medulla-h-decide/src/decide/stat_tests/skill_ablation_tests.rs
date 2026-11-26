//! Ablation, noise band, canary and compile staleness served through `DecisionLog` `learn` and
//! `Decide` kind `Promotion` (MEDULLA-DECISION-ENGINE-R712.3 to R712.5).

use super::learning_tests::{activate, graph_with, pointer_state};
use super::path_preference_tests::pins;
use super::promotion_tests::{commit, promotion_request, rollout_set};
use super::retrieval_tests::learn;
use super::*;
use medulla_decision_outcome_types::decision::promotion::RevisionSubject;
use medulla_decision_stats::decision::promotion::rollout_set_digest;
use medulla_decision_types::decision::codes::{
    ABLATION_NOISE_BAND_MISSING, ABLATION_WITHIN_NOISE, CANARY_NO_CHAMPION,
    CANARY_TRAFFIC_OUT_OF_BOUNDS, COMPILE_MODEL_UNKNOWN,
};
use medulla_decision_types::decision::skill_evolution::RolloutSet;
use medulla_decision_types::decision::statistical::retrieval::{
    LearningRecorded, LearningWrite, PointerRef,
};
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::skill_eval::{
    AblationArm, AblationRecord, AblationRun, AblationVerdict, CanaryVerdict, CompileState,
    ModelCompile, SkillCanary,
};

const SKILL: &str = "skill:triage@3";

fn subject() -> RevisionSubject {
    RevisionSubject::Skill {
        skill_ref: SKILL.to_owned(),
    }
}

/// Sixty paired rollouts of the skill: `challenger` against `champion`.
fn skill_set(challenger: &str, champion: &str, gain_ppm: u32) -> RolloutSet {
    RolloutSet {
        program_ref: SKILL.to_owned(),
        challenger_ref: challenger.to_owned(),
        champion_ref: Some(champion.to_owned()),
        ..rollout_set(gain_ppm)
    }
}

async fn record_set(h: &Harness, set: &RolloutSet) -> String {
    learn(h, "decider", LearningWrite::RecordRollouts { set: Box::new(set.clone()) })
        .await
        .unwrap();
    rollout_set_digest(set)
}

/// Make `revision:one` the champion of the skill through a committed `promote` record.
async fn champion(h: &Harness, schema: &medulla_decision_types::decision::ComponentDependency) -> String {
    let set = skill_set("revision:one", "revision:zero", 150_000);
    let digest = record_set(h, &set).await;
    let batch = decide(h, promotion_request(schema, &subject(), &digest, "revision:one"))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    commit(h, &record).await;
    pointer_state(h, PointerRef::Champion { subject: subject() }, activate("revision:one", &record.record_id))
        .await
        .unwrap();
    digest
}

fn canary_request(
    schema: &medulla_decision_types::decision::ComponentDependency,
    receipt: &str,
    extra: &[(&str, TypedValue)],
) -> DecideRequest {
    let mut request = promotion_request(schema, &subject(), receipt, "revision:two");
    let mut params: Vec<TypedParam> = request
        .params
        .as_slice()
        .iter()
        .filter(|param| param.name != "mode")
        .cloned()
        .collect();
    params.push(TypedParam {
        name: "mode".to_owned(),
        value: TypedValue::Text("canary".to_owned()),
    });
    for (name, value) in extra {
        params.push(TypedParam {
            name: (*name).to_owned(),
            value: value.clone(),
        });
    }
    request.params = BoundedVec::new(params).unwrap();
    request
}

fn ablation(identical: Vec<u64>, gain: u64) -> LearningWrite {
    let runs = (0..60u64)
        .flat_map(|index| {
            [(AblationArm::With, 600_000 + gain), (AblationArm::Without, 600_000)]
                .into_iter()
                .map(move |(arm, score)| AblationRun {
                    skill_ref: SKILL.into(),
                    prompt_digest: format!("prompt-{index}"),
                    arm,
                    blind_id: format!("blind-{index}-{score}"),
                    outcome_score_ppm: score,
                    seed: index % 3,
                })
        })
        .collect();
    LearningWrite::RecordAblation {
        record: Box::new(AblationRecord {
            skill_ref: SKILL.into(),
            identical_scores_ppm: identical,
            runs,
        }),
    }
}

fn facts(batch: &DecisionBatch) -> Vec<String> {
    batch.records.as_slice()[0].premises.iter().map(|p| p.fact.clone()).collect()
}

fn abstain_code(batch: &DecisionBatch) -> String {
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("expected an abstention")
    };
    reasons.as_slice()[0].code().as_str().to_owned()
}

// spec: MEDULLA-DECISION-ENGINE-R712.3
#[tokio::test]
async fn canary_decides_ramp_hold_or_rollback() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    champion(&h, &pins.0).await;
    // Five identical runs spread by a few thousand ppm; the paired runs gain 100k.
    let recorded = learn(&h, "decider", ablation(vec![700_000, 702_000, 698_000, 701_000, 699_000], 100_000))
        .await
        .unwrap();
    let LearningRecorded::Ablation(recorded) = recorded else { panic!("ablation answers") };
    assert_eq!(recorded.noise.identical_runs, 5);
    assert_eq!(recorded.verdict, AblationVerdict::Effect { delta_ppm: 100_000 });

    // A challenger far outside the band ramps, one inside it holds.
    let winning = record_set(&h, &skill_set("revision:two", "revision:one", 150_000)).await;
    let ramp = decide(&h, canary_request(&pins.0, &winning, &[])).await.unwrap();
    assert!(facts(&ramp).contains(&"canary:verdict=ramp".to_owned()), "{:?}", facts(&ramp));
    let flat = record_set(&h, &skill_set("revision:two", "revision:one", 0)).await;
    let hold = decide(&h, canary_request(&pins.0, &flat, &[])).await.unwrap();
    assert!(facts(&hold).contains(&"canary:verdict=hold".to_owned()));

    // The ramp puts a canary in force, and the champion pointer read returns the split.
    let canary = SkillCanary {
        skill_ref: SKILL.into(),
        champion_revision: "revision:one".into(),
        challenger_revision: "revision:two".into(),
        traffic_ppm: 50_000,
        window_ms: 3_600_000,
    };
    learn(&h, "decider", LearningWrite::RecordCanary { canary: Box::new(canary.clone()), verdict: CanaryVerdict::Ramp })
        .await
        .unwrap();
    let pointer = super::super::stat_pointer::champion_pointer(
        &h.store,
        TENANT,
        &subject().pointer_key(),
    )
    .unwrap();
    assert_eq!(pointer.canary, Some(canary.clone()));

    // A later measurement of the champion that fell below its promotion rolls the canary back.
    let later = record_set(&h, &skill_set("revision:one", "revision:zero", 0)).await;
    let rollback = decide(
        &h,
        canary_request(&pins.0, &winning, &[("regression_receipt", TypedValue::Text(later))]),
    )
    .await
    .unwrap();
    assert!(facts(&rollback).contains(&"canary:verdict=rollback".to_owned()), "{:?}", facts(&rollback));
    learn(&h, "decider", LearningWrite::RecordCanary { canary: Box::new(canary), verdict: CanaryVerdict::Rollback })
        .await
        .unwrap();
    let cleared = super::super::stat_pointer::champion_pointer(&h.store, TENANT, &subject().pointer_key()).unwrap();
    assert_eq!(cleared.canary, None);
}

// spec: MEDULLA-DECISION-ENGINE-R712.4
#[tokio::test]
async fn refusal_codes() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let receipt = record_set(&h, &skill_set("revision:two", "revision:one", 150_000)).await;

    // No champion yet.
    let none = decide(&h, canary_request(&pins.0, &receipt, &[])).await.unwrap();
    assert_eq!(abstain_code(&none), CANARY_NO_CHAMPION.as_str());

    champion(&h, &pins.0).await;
    // No identical runs recorded.
    let missing = decide(&h, canary_request(&pins.0, &receipt, &[])).await.unwrap();
    assert_eq!(abstain_code(&missing), ABLATION_NOISE_BAND_MISSING.as_str());
    // Fewer identical runs than the knob.
    let few = learn(&h, "decider", ablation(vec![1, 2, 3], 0)).await.unwrap_err();
    assert!(few.starts_with(ABLATION_NOISE_BAND_MISSING.as_str()), "{few}");

    // A widely spread band: a promotion on a small delta is refused.
    learn(&h, "decider", ablation(vec![500_000, 700_000, 600_000, 800_000, 550_000], 200_000))
        .await
        .unwrap();
    let small = record_set(&h, &skill_set("revision:three", "revision:one", 5_000)).await;
    let within = decide(&h, promotion_request(&pins.0, &subject(), &small, "revision:three"))
        .await
        .unwrap();
    assert_eq!(abstain_code(&within), ABLATION_WITHIN_NOISE.as_str());
    // A traffic share below its bound.
    let low = decide(&h, canary_request(&pins.0, &receipt, &[("traffic_ppm", TypedValue::Int(500))]))
        .await
        .unwrap();
    assert_eq!(abstain_code(&low), CANARY_TRAFFIC_OUT_OF_BOUNDS.as_str());

    // A model the catalog lacks.
    graph_with(&h, "catalog", "1", 3, Vec::new()).await;
    let unknown = learn(
        &h,
        "decider",
        LearningWrite::RecordModelCompile {
            graph: "catalog".into(),
            compile: Box::new(ModelCompile {
                skill_ref: SKILL.into(),
                model_ref: "model:none".into(),
                compiled_digest: "d1".into(),
                state: CompileState::Current,
            }),
            model_revision: "r1".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(unknown.starts_with(COMPILE_MODEL_UNKNOWN.as_str()), "{unknown}");
}

// spec: MEDULLA-DECISION-ENGINE-R712.5
#[tokio::test]
async fn premises_name_band_and_split() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    champion(&h, &pins.0).await;
    learn(&h, "decider", ablation(vec![700_000, 702_000, 698_000, 701_000, 699_000], 100_000))
        .await
        .unwrap();
    let receipt = record_set(&h, &skill_set("revision:two", "revision:one", 150_000)).await;

    // A model revision change marks the other compiled rows of the model stale.
    let core = graph_with(&h, "catalog", "1", 3, Vec::new()).await;
    core.add_node(
        "model:m1".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({"type": "Model"})).unwrap(),
    );
    for (skill, revision) in [(SKILL, "r1"), ("skill:other@1", "r1"), ("skill:third@1", "r2")] {
        let recorded = learn(
            &h,
            "decider",
            LearningWrite::RecordModelCompile {
                graph: "catalog".into(),
                compile: Box::new(ModelCompile {
                    skill_ref: skill.into(),
                    model_ref: "model:m1".into(),
                    compiled_digest: format!("d-{skill}"),
                    state: CompileState::Stale,
                }),
                model_revision: revision.into(),
            },
        )
        .await
        .unwrap();
        let LearningRecorded::ModelCompile(recorded) = recorded else { panic!("compile answers") };
        assert_eq!(recorded.compile.state, CompileState::Current);
        if revision == "r2" {
            // The two rows of revision r1 are now stale, each with one draft proposal.
            assert_eq!(recorded.stale.len(), 2);
            assert_eq!(recorded.proposals.len(), 2);
        }
    }

    let batch = decide(&h, canary_request(&pins.0, &receipt, &[])).await.unwrap();
    let facts = facts(&batch);
    for wanted in [
        "ablation:band=",
        "ablation:delta=",
        "canary:traffic=50000",
        "canary:verdict=ramp",
        "compile:stale=1",
    ] {
        assert!(facts.iter().any(|fact| fact.starts_with(wanted)), "{wanted} in {facts:?}");
    }
    // No premise names the arm of a blind run.
    assert!(facts.iter().all(|fact| !fact.contains("without") && !fact.contains("blind")));
}
