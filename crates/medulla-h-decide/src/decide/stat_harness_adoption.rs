//! `DecisionLog.harness_adoption`: the one write authority of the adopted
//! harness, its behavioral contrast and its lesson (MEDULLA-DECISION-ENGINE-R803.8).
//!
//! The write names a committed `HarnessContrast` decision by its record
//! digest. The handler reads the contrast deltas from that decision (the
//! request never carries them), checks that the decision adopted the named
//! version under the named conditions, then writes the three records, their
//! supersession edges and the idempotency row as ONE owner write
//! (`put_decision_artifacts`). The adoption supersedes the previous adoption
//! of the same agent and task class by edge; both stay readable.

use medulla_decision_types::decision::statistical::log::{
    HarnessAdoptionStored, HarnessAdoptionWrite,
};
use medulla_decision_types::decision::statistical::QuestionKind;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::BoundedVec;
use medulla_work_types::orchestration::improvement::{
    register_adoption, register_lesson, AdoptedHarness, AdoptionSupersession, HarnessContrast,
    HarnessLesson, HarnessProbeRun, ImprovementLibraryRefusal, LessonEvidence, LessonSupersession,
};
use serde::de::DeserializeOwned;

use super::stat_executor::ExecutionContext;
use super::stat_log::{visible_entries, LogReader};

/// The most rows of one record family a tenant's library read returns.
const MAX_LIBRARY_ROWS: usize = 100_000;
const ADOPTED_PREFIX: &str = "harness-adopted:";
const LESSON_PREFIX: &str = "harness-lesson:";
const CONTRAST_PREFIX: &str = "harness-contrast:";
const ADOPTION_EDGE_PREFIX: &str = "harness-supersession:adoption:";
const LESSON_EDGE_PREFIX: &str = "harness-supersession:lesson:";
const WRITE_PREFIX: &str = "harness-adoption-write:";

fn refuse(refusal: ImprovementLibraryRefusal, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", refusal.code())
}

/// Every stored row of one family, in key (write) order.
fn load<T: DeserializeOwned>(
    ctx: &ExecutionContext,
    prefix: &str,
    noun: &str,
) -> Result<Vec<T>, String> {
    ctx.store
        .decision_artifacts_with_prefix(ctx.tenant_id, prefix, MAX_LIBRARY_ROWS)?
        .into_iter()
        .map(|(_, bytes)| decode_artifact::<T>(&bytes, noun))
        .collect()
}

/// A write-ordered key: rows of one family list in the order they were written.
fn ordered_key(prefix: &str, now_ms: u64, node_id: &str) -> String {
    format!("{prefix}{now_ms:020}:{node_id}")
}

/// The contrast the committed decision measured for the adopted candidate.
fn adopted_contrast(
    write: &HarnessAdoptionWrite,
    ctx: &ExecutionContext,
    reader: &LogReader,
) -> Result<(String, HarnessContrast), String> {
    let no_decision =
        |detail: &str| refuse(ImprovementLibraryRefusal::AdoptedHarnessNoDecision, detail);
    let entry = visible_entries(ctx.store, reader)?
        .into_iter()
        .find(|entry| entry.record.record_digest == write.decision_record_digest)
        .ok_or_else(|| no_decision("no committed record with that digest is visible"))?;
    let record = &entry.record;
    let decision = match (&record.question.kind, &record.inputs.harness_contrast) {
        (QuestionKind::HarnessContrast, Some(decision)) => decision,
        _ => {
            return Err(no_decision(
                "the named record is not a harness contrast decision",
            ))
        }
    };
    if decision.adopted_version_ref.as_deref() != Some(write.adopted_version_ref.as_str())
        || decision.task_ref != write.task_ref
        || decision.task_class != write.task_class
    {
        return Err(no_decision(
            "the named decision did not adopt this version for this task",
        ));
    }
    let index = decision
        .candidate_version_refs
        .iter()
        .position(|candidate| *candidate == write.adopted_version_ref)
        .ok_or_else(|| no_decision("the adopted version is not a candidate of the decision"))?;
    let contrast = decision.deltas.as_slice()[index].clone();
    if decision.conditions_digest != write.conditions_digest
        || contrast.conditions_digest != write.conditions_digest
    {
        return Err(refuse(
            ImprovementLibraryRefusal::ContrastConditionsMismatch,
            "the write's conditions digest differs from the committed contrast",
        ));
    }
    Ok((record.record_id.clone(), contrast))
}

fn lesson_of(write: &HarnessAdoptionWrite) -> HarnessLesson {
    HarnessLesson {
        task_class: write.task_class.clone(),
        deficiency: write.deficiency.clone(),
        situation: write.situation.clone(),
        changed_component_kind: write.changed_component_kind,
        what_changed: write.what_changed.clone(),
        applicability: write.applicability.clone(),
        confidence: f64::from(write.confidence_ppm) / 1_000_000.0,
        evidence_run_ids: write.evidence_run_ids.clone(),
        supersedes_id: None,
    }
}

/// Serve one `HarnessAdoption` op.
pub(super) fn adopt(
    ctx: &ExecutionContext,
    reader: &LogReader,
    write: &HarnessAdoptionWrite,
) -> Result<HarnessAdoptionStored, String> {
    let write_key = format!(
        "{WRITE_PREFIX}{}:{}",
        write.task_ref, write.decision_record_digest
    );
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &write_key)? {
        let mut stored: HarnessAdoptionStored = decode_artifact(&bytes, "harness adoption")?;
        stored.replayed = true;
        return Ok(stored);
    }
    let (decision_ref, contrast) = adopted_contrast(write, ctx, reader)?;

    // The lesson first: its evidence refusal must not leave an adoption behind.
    let run_a = HarnessProbeRun {
        trace_id: contrast.run_a_trace_id.clone(),
        conditions_digest: contrast.conditions_digest.clone(),
    };
    let run_b = HarnessProbeRun {
        trace_id: contrast.run_b_trace_id.clone(),
        conditions_digest: contrast.conditions_digest.clone(),
    };
    let mut lessons: Vec<HarnessLesson> = load(ctx, LESSON_PREFIX, "harness lesson")?;
    let mut lesson_edges: Vec<LessonSupersession> =
        load(ctx, LESSON_EDGE_PREFIX, "lesson supersession")?;
    let (lessons_before, lesson_edges_before) = (lessons.len(), lesson_edges.len());
    let lesson = lesson_of(write);
    let lesson_ref = lesson.node_id();
    register_lesson(
        &mut lessons,
        &mut lesson_edges,
        lesson,
        &LessonEvidence {
            contrast: &contrast,
            run_a: &run_a,
            run_b: &run_b,
        },
        ctx.now_ms,
        ctx.now_ms,
    )
    .map_err(|refusal| refuse(refusal, "the lesson is not admitted"))?;

    let adopted = AdoptedHarness {
        agent_ref: write.agent_ref.clone(),
        task_class: write.task_class.clone(),
        component_version_refs: BoundedVec::new(vec![write.adopted_version_ref.clone()])
            .map_err(|detail| refuse(ImprovementLibraryRefusal::RecordMalformed, detail))?,
        decision_ref,
        contrast_ref: Some(contrast.node_id()),
    };
    let adopted_ref = adopted.node_id();
    let mut adoptions: Vec<AdoptedHarness> = load(ctx, ADOPTED_PREFIX, "adopted harness")?;
    let mut adoption_edges: Vec<AdoptionSupersession> =
        load(ctx, ADOPTION_EDGE_PREFIX, "adoption supersession")?;
    let (adoptions_before, adoption_edges_before) = (adoptions.len(), adoption_edges.len());
    register_adoption(
        &mut adoptions,
        &mut adoption_edges,
        adopted,
        ctx.now_ms,
        ctx.now_ms,
    )
    .map_err(|refusal| refuse(refusal, "the adoption is not admitted"))?;

    let supersedes_ref = adoption_edges[adoption_edges_before..]
        .iter()
        .find(|edge| edge.new_version == adopted_ref && !edge.superseded_version.is_empty())
        .map(|edge| edge.superseded_version.clone());
    let stored = HarnessAdoptionStored {
        adopted_harness_ref: adopted_ref.clone(),
        contrast_ref: contrast.node_id(),
        lesson_ref: lesson_ref.clone(),
        supersedes_ref,
        replayed: false,
    };

    // One owner write: the three records, the new edges and the idempotency row.
    let mut rows: Vec<(String, Vec<u8>)> = vec![(
        format!("{CONTRAST_PREFIX}{}", contrast.node_id()),
        encode_artifact(&contrast)?,
    )];
    if adoptions.len() > adoptions_before {
        rows.push((
            ordered_key(ADOPTED_PREFIX, ctx.now_ms, &adopted_ref),
            encode_artifact(&adoptions[adoptions_before])?,
        ));
    }
    for edge in &adoption_edges[adoption_edges_before..] {
        rows.push((
            format!("{ADOPTION_EDGE_PREFIX}{}", edge.new_version),
            encode_artifact(edge)?,
        ));
    }
    if lessons.len() > lessons_before {
        rows.push((
            ordered_key(LESSON_PREFIX, ctx.now_ms, &lesson_ref),
            encode_artifact(&lessons[lessons_before])?,
        ));
    }
    for edge in &lesson_edges[lesson_edges_before..] {
        rows.push((
            format!("{LESSON_EDGE_PREFIX}{}", edge.new_version),
            encode_artifact(edge)?,
        ));
    }
    rows.push((write_key, encode_artifact(&stored)?));
    ctx.store.put_decision_artifacts(ctx.tenant_id, &rows)?;
    Ok(stored)
}
