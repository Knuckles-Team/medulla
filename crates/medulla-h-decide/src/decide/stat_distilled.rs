//! Distilled scores, stored and ranked (MEDULLA-DECISION-ENGINE-R147.3 to R147.5).
//!
//! A teacher's judgments are computed offline and uploaded through
//! `DecisionLog` `Learn`; `Decide` ranks graph candidates from the stored
//! integers and makes no model call. Nothing here scores a candidate at
//! query time: a candidate without a stored score refuses.
//!
//! Stored rows (decision artifact store, one tenant):
//! * `distilled/<scorer_id>/latest` -- the newest uploaded revision.
//! * `distilled/<scorer_id>/<revision>/<batch digest hex>` -- one batch of the
//!   judgments new to the scorer, written once.
//! * `distilled/<scorer_id>/<revision>/manifest` -- the manifest, replaced by
//!   compare-and-set on every batch. Its `score_digest` is the SHA-256 of the
//!   previous digest bytes (32 zero bytes at first) followed by the batch
//!   digest bytes.

use std::collections::BTreeMap;

use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::distilled::{rank_stored, MissingScore};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::statistical::distilled::{
    DistilledScorerManifest, DistilledUpload, TeacherJudgment, KNOBS, Q16_ONE,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalErrorCode,
    StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, DecisionPolicyRef, PremiseClass, QuantScaleTag, QuantisedValue,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::{BoundedVec, Digest256};

use super::family::advisory;
use super::stat_executor::{Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource};
use super::stat_support::{refusal, resolve_policy};

/// The request parameter that selects distilled ranking.
const PARAM: &str = "distilled_scorer";
const MANIFEST: &str = "manifest";
/// More batch rows than this under one scorer revision refuse the read.
const MAX_BATCH_ROWS: usize = 65_536;

fn prefix(scorer_id: &str, revision: u64) -> String {
    format!("distilled/{scorer_id}/{revision}/")
}

fn latest_key(scorer_id: &str) -> String {
    format!("distilled/{scorer_id}/latest")
}

fn manifest_key(scorer_id: &str, revision: u64) -> String {
    format!("{}{MANIFEST}", prefix(scorer_id, revision))
}

fn knob_int(policy: &DecisionPolicy, index: usize) -> Result<i64, String> {
    match policy.knob(&KNOBS[index]) {
        KnobValue::Integer(value) => Ok(value),
        _ => Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "distilled knobs have the wrong value type",
        )),
    }
}

fn knob_micros(policy: &DecisionPolicy, index: usize) -> Result<i64, String> {
    match policy.knob(&KNOBS[index]) {
        KnobValue::DecimalMicros(value) => Ok(value),
        _ => Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "distilled knobs have the wrong value type",
        )),
    }
}

fn plain_id(scorer_id: &str) -> Result<(), String> {
    if scorer_id.is_empty() || scorer_id.contains('/') {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a scorer id is non-empty and holds no `/`",
        ));
    }
    Ok(())
}

fn stored_manifest(
    store: &AgentLibraryStore,
    tenant: &str,
    scorer_id: &str,
    revision: u64,
) -> Result<(Option<Vec<u8>>, Option<DistilledScorerManifest>), String> {
    let bytes = store.decision_artifact(tenant, &manifest_key(scorer_id, revision))?;
    let manifest = bytes
        .as_deref()
        .map(|bytes| decode_artifact(bytes, "distilled scorer manifest"))
        .transpose()?;
    Ok((bytes, manifest))
}

fn latest_revision(
    store: &AgentLibraryStore,
    tenant: &str,
    scorer_id: &str,
) -> Result<(Option<Vec<u8>>, Option<u64>), String> {
    let bytes = store.decision_artifact(tenant, &latest_key(scorer_id))?;
    let revision = bytes
        .as_deref()
        .map(|bytes| decode_artifact::<u64>(bytes, "distilled latest revision"))
        .transpose()?;
    Ok((bytes, revision))
}

/// Every stored judgment of one scorer revision, by document id.
fn stored_scores(
    store: &AgentLibraryStore,
    tenant: &str,
    scorer_id: &str,
    revision: u64,
) -> Result<BTreeMap<String, TeacherJudgment>, String> {
    let manifest = manifest_key(scorer_id, revision);
    let mut scores = BTreeMap::new();
    for (key, bytes) in store.decision_artifacts_with_prefix(
        tenant,
        &prefix(scorer_id, revision),
        MAX_BATCH_ROWS,
    )? {
        if key == manifest {
            continue;
        }
        let batch: Vec<TeacherJudgment> = decode_artifact(&bytes, "distilled score batch")?;
        for judgment in batch {
            scores.insert(judgment.document_id.clone(), judgment);
        }
    }
    Ok(scores)
}

/// Store one batch of judgments and chain the manifest digest
/// (MEDULLA-DECISION-ENGINE-R147.3). A batch that repeats stored judgments
/// exactly changes nothing; a different score for a stored document refuses.
pub(super) fn upload_scores(
    ctx: &ExecutionContext,
    upload: DistilledUpload,
) -> Result<DistilledScorerManifest, String> {
    let policy = resolve_policy(ctx.store, ctx.tenant_id, &DecisionPolicyRef::Default)?;
    let max_scores = usize::try_from(knob_int(&policy.policy, 1)?).unwrap_or(0);
    plain_id(&upload.scorer_id)?;
    let count = upload.scores.len();
    if count == 0 || count > max_scores {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("an upload holds 1 to {max_scores} scores, not {count}"),
        ));
    }
    let mut batch: BTreeMap<String, TeacherJudgment> = BTreeMap::new();
    for judgment in upload.scores.iter().cloned() {
        let id = judgment.document_id.clone();
        let judgment = judgment.checked().map_err(|_| {
            refusal(
                StatisticalErrorCode::DistilledScoreInvalid,
                format!("the score of {id} is out of range or its bounds are inverted"),
            )
        })?;
        match batch.insert(id.clone(), judgment.clone()) {
            Some(earlier) if earlier != judgment => {
                return Err(refusal(
                    StatisticalErrorCode::IdempotencyConflict,
                    format!("the upload scores {id} twice with different values"),
                ));
            }
            _ => {}
        }
    }
    let (latest_bytes, latest) = latest_revision(ctx.store, ctx.tenant_id, &upload.scorer_id)?;
    if latest.is_some_and(|latest| upload.scorer_revision < latest) {
        return Err(refusal(
            StatisticalErrorCode::ScorerRevisionStale,
            format!(
                "scorer {} is at revision {}, not {}",
                upload.scorer_id,
                latest.unwrap_or_default(),
                upload.scorer_revision
            ),
        ));
    }
    let stored = stored_scores(
        ctx.store,
        ctx.tenant_id,
        &upload.scorer_id,
        upload.scorer_revision,
    )?;
    let mut fresh = Vec::new();
    for (id, judgment) in batch {
        match stored.get(&id) {
            Some(existing) if *existing == judgment => {}
            Some(_) => {
                return Err(refusal(
                    StatisticalErrorCode::IdempotencyConflict,
                    format!("{id} is already stored with another score"),
                ));
            }
            None => fresh.push(judgment),
        }
    }
    let (manifest_bytes, manifest) = stored_manifest(
        ctx.store,
        ctx.tenant_id,
        &upload.scorer_id,
        upload.scorer_revision,
    )?;
    if fresh.is_empty() {
        if let Some(manifest) = manifest {
            return Ok(manifest);
        }
    }
    let batch_bytes = encode_artifact(&fresh)?;
    let batch_digest = Digest256::sha256(&batch_bytes);
    let previous = manifest
        .as_ref()
        .map(|m| *m.score_digest.as_bytes())
        .unwrap_or([0u8; 32]);
    let mut chained = previous.to_vec();
    chained.extend_from_slice(batch_digest.as_bytes());
    let next = DistilledScorerManifest {
        scorer_id: upload.scorer_id.clone(),
        scorer_revision: upload.scorer_revision,
        score_count: manifest.as_ref().map_or(0, |m| m.score_count) + fresh.len() as u64,
        score_digest: Digest256::sha256(&chained),
        persisted_at_ms: ctx.now_ms,
    };
    let batch_key = format!(
        "{}{}",
        prefix(&upload.scorer_id, upload.scorer_revision),
        batch_digest.to_hex()
    );
    ctx.store
        .put_decision_artifacts(ctx.tenant_id, &[(batch_key, batch_bytes)])?;
    ctx.store.swap_decision_artifact(
        ctx.tenant_id,
        &manifest_key(&upload.scorer_id, upload.scorer_revision),
        manifest_bytes,
        encode_artifact(&next)?,
    )?;
    if latest != Some(upload.scorer_revision) {
        ctx.store.swap_decision_artifact(
            ctx.tenant_id,
            &latest_key(&upload.scorer_id),
            latest_bytes,
            encode_artifact(&upload.scorer_revision)?,
        )?;
    }
    Ok(next)
}

/// The Q32 score of a Q16 judgment.
fn q32(score_q16: u32) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: i64::from(score_q16) << 16,
    }
}

fn premise(fact: String, source: FamilyPremiseSource) -> FamilyPremise {
    FamilyPremise {
        subject: "distilled",
        fact,
        class: PremiseClass::Claim,
        source,
    }
}

/// Rank the visible graph candidates of a `rank` question from stored scores,
/// when the request carries the `distilled_scorer` parameter; `None` for every
/// other request. Every refusal precedes ranking and writes nothing.
pub(super) fn rank(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    views: &[CandidateView],
) -> Result<Option<Executed>, String> {
    if request.question.kind != QuestionKind::Rank {
        return Ok(None);
    }
    let mut found = request.params.iter().filter(|p| p.name == PARAM);
    let Some(param) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("parameter {PARAM} is given more than once"),
        ));
    }
    let TypedValue::DistilledScorer {
        scorer_id,
        scorer_revision,
        top_k,
    } = &param.value
    else {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("parameter {PARAM} is not a distilled scorer"),
        ));
    };
    if !matches!(request.candidates, CandidateSource::Graph { .. }) {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "distilled ranking takes its candidates from a graph source",
        ));
    }
    plain_id(scorer_id)?;
    let max_top_k = knob_int(policy, 0)?;
    if *top_k == 0 || i64::from(*top_k) > max_top_k {
        return Err(refusal(
            StatisticalErrorCode::TopKInvalid,
            format!("top_k {top_k} is outside 1 to {max_top_k}"),
        ));
    }
    let (_, latest) = latest_revision(ctx.store, ctx.tenant_id, scorer_id)?;
    if latest.is_some_and(|latest| latest != *scorer_revision) {
        return Err(refusal(
            StatisticalErrorCode::ScorerRevisionStale,
            format!(
                "scorer {scorer_id} is stored at revision {}, not {scorer_revision}",
                latest.unwrap_or_default()
            ),
        ));
    }
    let (_, manifest) = stored_manifest(ctx.store, ctx.tenant_id, scorer_id, *scorer_revision)?;
    let Some(manifest) = manifest.filter(|m| m.score_count > 0) else {
        return Err(refusal(
            StatisticalErrorCode::DistilledScorerEmpty,
            format!("scorer {scorer_id}@{scorer_revision} holds no stored score"),
        ));
    };
    let scores = stored_scores(ctx.store, ctx.tenant_id, scorer_id, *scorer_revision)?;
    let candidates: Vec<String> = views.iter().map(|view| view.id.clone()).collect();
    let ranked =
        rank_stored(&candidates, &scores, candidates.len()).map_err(|MissingScore(document)| {
            refusal(
                StatisticalErrorCode::DistilledScoreMissing,
                format!("candidate {document} has no stored score"),
            )
        })?;
    let floor_micros = knob_micros(policy, 2)?;
    // Compare lo/65536 against floor/1_000_000 without division.
    if let Some(low) = ranked.iter().find(|judgment| {
        i64::from(judgment.fidelity_lo_q16) * 1_000_000 < floor_micros * i64::from(Q16_ONE)
    }) {
        return Err(refusal(
            StatisticalErrorCode::DistilledFidelityBelowFloor,
            format!(
                "the score of {} has a low fidelity bound of {} (Q16), under the floor",
                low.document_id, low.fidelity_lo_q16
            ),
        ));
    }
    let fidelity_min = ranked
        .iter()
        .map(|judgment| judgment.fidelity_lo_q16)
        .min()
        .unwrap_or(0);
    let kept: Vec<ScoredOption> = ranked
        .iter()
        .take(*top_k as usize)
        .map(|judgment| ScoredOption {
            option_id: judgment.document_id.clone(),
            score: q32(judgment.score_q16),
            probability: None,
        })
        .collect();
    let manifest_ref = manifest_key(scorer_id, *scorer_revision);
    let from_manifest = || FamilyPremiseSource::Observation(manifest_ref.clone());
    let mut premises = vec![
        premise(
            format!(
                "distilled:scorer={}@{}",
                manifest.scorer_id, manifest.scorer_revision
            ),
            from_manifest(),
        ),
        premise(
            format!("distilled:digest={}", manifest.score_digest.to_hex()),
            from_manifest(),
        ),
        premise(
            format!("distilled:score_count={}", manifest.score_count),
            from_manifest(),
        ),
        premise(
            format!("distilled:persisted_at_ms={}", manifest.persisted_at_ms),
            from_manifest(),
        ),
        premise(
            format!("distilled:fidelity_lo_min_q16={fidelity_min}"),
            from_manifest(),
        ),
        premise("distilled:model_calls=0".to_string(), from_manifest()),
    ];
    for (index, spec) in KNOBS.iter().enumerate() {
        premises.push(FamilyPremise {
            subject: "policy",
            fact: format!("knob:{}={}", spec.name, policy.knob(&KNOBS[index]).render()),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
    }
    let mut executed = advisory(Vec::new())?;
    if let StatisticalOutcome::Advisory { scores, .. } = &mut executed.ladder.outcome {
        *scores = BoundedVec::new(kept)
            .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    }
    executed.family_premises = premises;
    Ok(Some(executed))
}
