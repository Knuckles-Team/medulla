//! The ablation and compile writes behind `DecisionLog` `learn` (MEDULLA-DECISION-ENGINE-R712.3).
//!
//! `record_ablation` turns the identical runs of one skill into its noise band and the paired
//! with-and-without runs into a stored rollout set, then judges the pairs against the band.
//! `record_model_compile` records the program a skill was compiled into for one model and, when
//! the model revision changed, marks the other rows of that model stale and files one draft
//! revision per stale row. Both stop at a refusal before anything is stored.

use std::collections::BTreeMap;

use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
use medulla_decision_stats::decision::skill_eval::{
    ablation_verdict, check_traffic, noise_band, stale_after_change,
};
use medulla_decision_types::decision::codes::CANARY_NO_CHAMPION;
use medulla_persistence_records::decision_jobs::read_pointer;
use medulla_decision_types::decision::codes::COMPILE_MODEL_UNKNOWN;
use medulla_decision_types::decision::skill_evolution::{Rollout, RolloutSet};
use medulla_decision_types::skill_eval::{
    AblationArm, AblationRecord, AblationRecorded, CanaryRecorded, CanaryVerdict, CompileState,
    ModelCompile, ModelCompileRecorded, NoiseBand, SkillCanary, CANARY_TRAFFIC_MAX_PPM,
    CANARY_TRAFFIC_MIN_PPM,
};
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_types::contract::BoundedVec;

use super::stat_executor::ExecutionContext;
use super::stat_promotion::record_rollouts;
use super::stat_support::invalid_parameter;
use super::stat_vectors::GraphVectors;

/// The store key of the noise band of one skill.
const NOISE_PREFIX: &str = "noise-band:";
/// The store key prefix of the compiled rows, `model-compile:<model_ref>:<skill_ref>`.
const COMPILE_PREFIX: &str = "model-compile:";
/// The store key prefix of the model revision in force, `model-revision:<model_ref>`.
const REVISION_PREFIX: &str = "model-revision:";
/// Most compiled rows of one model that a change marks stale.
const MAX_COMPILE_ROWS: usize = 10_000;

/// The store key of the noise band of `skill_ref`.
pub(super) fn noise_key(skill_ref: &str) -> String {
    format!("{NOISE_PREFIX}{skill_ref}")
}

/// The store key of the canary split of one skill.
fn canary_key(skill_ref: &str) -> String {
    format!("canary:{skill_ref}")
}

/// The canary split in force for `skill_ref`, when there is one.
pub(super) fn read_canary(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
) -> Result<Option<SkillCanary>, String> {
    Ok(store
        .decision_artifact(tenant, &canary_key(skill_ref))?
        .map(|bytes| decode_artifact::<Option<SkillCanary>>(&bytes, "canary"))
        .transpose()?
        .flatten())
}

/// The noise band recorded for `skill_ref`, when there is one.
pub(super) fn read_noise_band(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
) -> Result<Option<NoiseBand>, String> {
    store
        .decision_artifact(tenant, &noise_key(skill_ref))?
        .map(|bytes| decode_artifact(&bytes, "noise band"))
        .transpose()
}

fn refuse(code: medulla_types::refusal_code::RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// Record one ablation: the noise band, the paired rollouts and the verdict.
pub(super) fn record_ablation(
    ctx: &ExecutionContext,
    record: AblationRecord,
) -> Result<AblationRecorded, String> {
    if record.runs.iter().any(|run| run.skill_ref != record.skill_ref) {
        return Err(invalid_parameter(format!(
            "every run tests {}",
            record.skill_ref
        )));
    }
    let min_runs = medulla_server_api::knob_config::knobs().eval_ablation_identical_runs;
    let noise = noise_band(&record.identical_scores_ppm, min_runs)
        .map_err(|refusal| refuse(refusal.code, refusal.reason))?;

    // A pair is the with-arm run and the without-arm run of one prompt under one seed.
    let mut pairs: BTreeMap<(&str, u64), (Option<&_>, Option<&_>)> = BTreeMap::new();
    for run in &record.runs {
        let slot = pairs
            .entry((run.prompt_digest.as_str(), run.seed))
            .or_default();
        match run.arm {
            AblationArm::With => slot.0 = Some(run),
            AblationArm::Without => slot.1 = Some(run),
            AblationArm::WithoutPart { .. } => {}
        }
    }
    let mut with_scores = Vec::new();
    let mut without_scores = Vec::new();
    let mut rollouts = Vec::new();
    for ((prompt, seed), (with, without)) in &pairs {
        let (Some(with), Some(without)) = (with, without) else {
            continue;
        };
        let score = |value: u64| {
            u32::try_from(value)
                .ok()
                .filter(|ppm| *ppm <= 1_000_000)
                .ok_or_else(|| invalid_parameter("a run score is a ppm share, at most 1000000"))
        };
        let (challenger, champion) = (score(with.outcome_score_ppm)?, score(without.outcome_score_ppm)?);
        with_scores.push(u64::from(challenger));
        without_scores.push(u64::from(champion));
        rollouts.push(Rollout {
            seed: *seed,
            task_id: (*prompt).to_string(),
            challenger_score_ppm: Some(challenger),
            champion_score_ppm: Some(champion),
            trace_ref: with.blind_id.clone(),
        });
    }
    let verdict = ablation_verdict(&with_scores, &without_scores, noise.band_ppm);
    let set = RolloutSet {
        schema_version: 1,
        program_ref: record.skill_ref.clone(),
        challenger_ref: format!("{}#with", record.skill_ref),
        champion_ref: Some(format!("{}#without", record.skill_ref)),
        rollouts: BoundedVec::new(rollouts).map_err(invalid_parameter)?,
    };
    let evidence = record_rollouts(ctx, set)?;
    let key = noise_key(&record.skill_ref);
    let current = ctx.store.decision_artifact(ctx.tenant_id, &key)?;
    let bytes = encode_artifact(&noise)?;
    if current.as_deref() != Some(bytes.as_slice()) {
        ctx.store
            .swap_decision_artifact(ctx.tenant_id, &key, current, bytes)?;
    }
    Ok(AblationRecorded {
        noise,
        verdict,
        evidence,
    })
}

/// Record the program `compile.skill_ref` was compiled into for `compile.model_ref`.
pub(super) fn record_model_compile(
    ctx: &ExecutionContext,
    graph: &GraphVectors,
    compile: ModelCompile,
    model_revision: &str,
) -> Result<ModelCompileRecorded, String> {
    let catalogued = graph
        .visible_properties(&compile.model_ref)
        .and_then(|properties| {
            properties
                .get("type")
                .or_else(|| properties.get("node_type"))
                .and_then(|class| class.as_str().map(|class| class == "Model"))
        })
        .unwrap_or(false);
    if !catalogued {
        return Err(refuse(
            COMPILE_MODEL_UNKNOWN,
            format!("{} is not in the model catalog", compile.model_ref),
        ));
    }
    let revision_key = format!("{REVISION_PREFIX}{}", compile.model_ref);
    let known_revision: Option<String> = ctx
        .store
        .decision_artifact(ctx.tenant_id, &revision_key)?
        .map(|bytes| decode_artifact(&bytes, "model revision"))
        .transpose()?;
    // The first record of a model fixes its revision; a different revision is a change.
    let changed = known_revision
        .as_deref()
        .is_some_and(|known| known != model_revision);
    let prefix = format!("{COMPILE_PREFIX}{}:", compile.model_ref);
    let rows: Vec<ModelCompile> = ctx
        .store
        .decision_artifacts_with_prefix(ctx.tenant_id, &prefix, MAX_COMPILE_ROWS)?
        .into_iter()
        .map(|(_, bytes)| decode_artifact(&bytes, "model compile"))
        .collect::<Result<_, _>>()?;
    let stale = if changed {
        stale_after_change(&rows, &compile.model_ref, &compile.skill_ref)
    } else {
        Vec::new()
    };
    let current = ModelCompile {
        state: CompileState::Current,
        ..compile
    };
    let mut writes = vec![
        (
            format!("{prefix}{}", current.skill_ref),
            encode_artifact(&current)?,
        ),
        (revision_key, encode_artifact(&model_revision.to_string())?),
    ];
    let mut proposals = Vec::new();
    for row in &stale {
        writes.push((
            format!("{prefix}{}", row.skill_ref),
            encode_artifact(row)?,
        ));
        // One draft revision per stale row: the skill is compiled again for the new revision.
        let draft = RevisionRow {
            id: format!("compile:{}:{}:{model_revision}", row.skill_ref, row.model_ref),
            subject_kind: "skill".to_string(),
            subject_ref: row.skill_ref.clone(),
            stage: RevisionStage::Draft,
            digest: row.compiled_digest.clone(),
            evaluation_receipt_id: None,
            created_ms: ctx.now_ms,
        };
        writes.push((draft.key(), encode_artifact(&draft)?));
        proposals.push(draft.id);
    }
    ctx.store.put_decision_artifacts(ctx.tenant_id, &writes)?;
    Ok(ModelCompileRecorded {
        compile: current,
        stale,
        proposals,
    })
}

/// Put a canary split in force after `ramp` or `hold`, or take it off after `rollback`
/// (MEDULLA-DECISION-ENGINE-R712.3, R712.4). A canary needs a champion and a traffic share inside
/// the bounds of knob `canary.traffic_ppm`; a refusal stores nothing.
pub(super) fn record_canary(
    ctx: &ExecutionContext,
    canary: SkillCanary,
    verdict: CanaryVerdict,
) -> Result<CanaryRecorded, String> {
    let key = canary_key(&canary.skill_ref);
    let subject = medulla_decision_outcome_types::decision::promotion::RevisionSubject::Skill {
        skill_ref: canary.skill_ref.clone(),
        tier: None,
    };
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &subject.pointer_key())?;
    let champion = state.active.as_ref().and_then(|active| active.target.clone());
    if verdict != CanaryVerdict::Rollback {
        let Some(champion) = champion else {
            return Err(refuse(
                CANARY_NO_CHAMPION,
                format!("{} has no champion to run a canary against", canary.skill_ref),
            ));
        };
        if champion != canary.champion_revision {
            return Err(refuse(
                CANARY_NO_CHAMPION,
                "the canary names a champion revision that is not the active one",
            ));
        }
        check_traffic(canary.traffic_ppm, CANARY_TRAFFIC_MIN_PPM, CANARY_TRAFFIC_MAX_PPM)
            .map_err(|refusal| refuse(refusal.code, refusal.reason))?;
    }
    let stored = (verdict != CanaryVerdict::Rollback).then_some(canary);
    let current = ctx.store.decision_artifact(ctx.tenant_id, &key)?;
    let bytes = encode_artifact(&stored)?;
    if current.as_deref() != Some(bytes.as_slice()) {
        ctx.store
            .swap_decision_artifact(ctx.tenant_id, &key, current, bytes)?;
    }
    Ok(CanaryRecorded { canary: stored })
}
