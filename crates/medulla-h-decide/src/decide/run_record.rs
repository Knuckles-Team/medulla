//! `DecisionLog` run records (MEDULLA-DECISION-ENGINE-R135.3).
//!
//! The engine compiles a successful run into a stored program keyed by task
//! signature. The key is `run_record/compiled_program/<task_signature>` in the
//! tenant's decision artifact store. `Decide` reads it and writes nothing;
//! this handler is the single write authority.

use medulla_decision_types::decision::branch_fusion::{
    branch_record_key, branch_retrieval_key, BranchRecord, BranchRef, BranchRetrievalReceipt,
    BRANCH_DUPLICATE, BRANCH_READBACK_STALE,
};
use medulla_decision_types::decision::program_store::preplan::PrePlanError;
use medulla_decision_types::decision::program_store::{
    compile_program,
    insight::{insight_key, Insight, InsightError},
    preplan::{
        max_receipt_age_ms, pattern_key, verify_preplan, ManifestReading, PrePlanTestReceipt,
        ReusablePattern,
    },
    program_key, ProgramKnobs, RunRecord, RunRecordStored, StoredRun,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::task_plan::horizon::{
    HORIZON_CHECKPOINT_MISSING, HORIZON_PLAN_STALE,
};
use medulla_decision_types::decision::task_plan::{
    attach_horizons, check_horizon_checkpoint, checkpoint_key, compute_plan_digest, HorizonKnobs,
};

use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};

use super::stat_executor::ExecutionContext;
use super::stat_log::store_once;
use super::stat_retention::Retention;
use super::stat_support::{refusal, resolve_policy};

/// Commit one run record. A refused compile stores nothing; a retry with equal
/// content answers the stored row and other content under the key refuses
/// `IDEMPOTENCY_CONFLICT`.
pub(super) fn commit(
    ctx: &ExecutionContext,
    retention: &Retention,
    record: RunRecord,
) -> Result<RunRecordStored, String> {
    match record {
        RunRecord::CompiledProgram {
            trace,
            policy,
            reusable,
            plan_binding,
            preplan_receipt,
        } => {
            let resolved = resolve_policy(ctx.store, ctx.tenant_id, &policy)?;
            let knobs = ProgramKnobs::from_policy(&resolved.policy);
            let program = compile_program(&trace, &knobs)
                .map_err(|error| refusal(StatisticalErrorCode::ParameterInvalid, error))?;
            // MEDULLA-DECISION-ENGINE-R135.7: a refused promotion stores nothing, so the
            // prior library stays as it was.
            let promoted = if reusable {
                let binding = plan_binding.ok_or_else(|| PrePlanError::Missing.to_string())?;
                let manifest = read_manifest(ctx, retention, preplan_receipt.as_ref());
                let verdict = verify_preplan(
                    ctx.tenant_id,
                    preplan_receipt.as_ref(),
                    &binding,
                    manifest?,
                    max_receipt_age_ms(&resolved.policy),
                    &resolved.digest,
                )
                .map_err(|error| error.to_string())?;
                let receipt = preplan_receipt.ok_or_else(|| PrePlanError::Missing.to_string())?;
                Some(ReusablePattern {
                    task_signature: program.task_signature.clone(),
                    program_digest: program.program_digest.clone(),
                    source_run_id: program.source_run_id.clone(),
                    plan: binding,
                    receipt,
                    verdict,
                })
            } else {
                None
            };
            let key = program_key(&program.task_signature);
            let replayed = ctx.store.decision_artifact(ctx.tenant_id, &key)?.is_some();
            let program = store_once(ctx, key.clone(), "compiled program", program, |a, b| a == b)?;
            let pattern = match promoted {
                None => None,
                Some(pattern) => Some(store_once(
                    ctx,
                    pattern_key(&program.task_signature),
                    "reusable pattern",
                    pattern,
                    |a, b| a == b,
                )?),
            };
            Ok(RunRecordStored {
                key,
                stored: StoredRun::CompiledProgram { program, pattern },
                replayed,
            })
        }
        // MEDULLA-DECISION-ENGINE-R135.12/R135.13: an insight with no source run, or one that
        // breaks an invariant, stores nothing; the row is keyed by its context signature
        // digest and id, and a retry with equal content answers the stored row.
        // MEDULLA-DECISION-ENGINE-R134.11: a checkpoint of another plan revision, or
        // one that breaks the accounting rule, stores nothing; horizon n > 0 is
        // stored only after horizon n - 1 was.
        RunRecord::HorizonCheckpoint {
            plan,
            policy,
            checkpoint,
        } => {
            let resolved = resolve_policy(ctx.store, ctx.tenant_id, &policy)?;
            let derived = plan.horizons.as_ref().filter(|held| {
                let mut fresh = (*plan).clone();
                compute_plan_digest(&plan) == plan.plan_digest
                    && attach_horizons(&mut fresh, &HorizonKnobs::from_policy(&resolved.policy))
                        .is_ok()
                    && fresh.horizons.as_ref() == Some(*held)
            });
            let partition = derived.ok_or_else(|| {
                format!(
                    "{HORIZON_PLAN_STALE}: the plan's horizons are not those its policy derives"
                )
            })?;
            check_horizon_checkpoint(&checkpoint, partition)?;
            if checkpoint.horizon_index > 0 {
                let before = checkpoint_key(&checkpoint.plan_digest, checkpoint.horizon_index - 1);
                if ctx
                    .store
                    .decision_artifact(ctx.tenant_id, &before)?
                    .is_none()
                {
                    return Err(format!(
                        "{HORIZON_CHECKPOINT_MISSING}: horizon {} has no stored checkpoint",
                        checkpoint.horizon_index - 1
                    ));
                }
            }
            let key = checkpoint_key(&checkpoint.plan_digest, checkpoint.horizon_index);
            let replayed = ctx.store.decision_artifact(ctx.tenant_id, &key)?.is_some();
            let checkpoint = store_once(
                ctx,
                key.clone(),
                "horizon checkpoint",
                checkpoint,
                |a, b| a == b,
            )?;
            Ok(RunRecordStored {
                key,
                stored: StoredRun::HorizonCheckpoint { checkpoint },
                replayed,
            })
        }
        // MEDULLA-DECISION-ENGINE-R134.14: a record with no result, no digest or a
        // pre-assigned position stores nothing; other content under the key refuses
        // `BRANCH_DUPLICATE` and leaves the stored row as it was.
        RunRecord::BranchRecord { record } => {
            record.validate()?;
            let key = record.key();
            let replayed = match ctx.store.decision_artifact(ctx.tenant_id, &key)? {
                None => {
                    ctx.store.put_decision_artifacts(
                        ctx.tenant_id,
                        &[(key.clone(), encode_artifact(&record)?)],
                    )?;
                    false
                }
                Some(bytes) => {
                    let held: BranchRecord = decode_artifact(&bytes, "branch record")?;
                    if held != record {
                        return Err(format!(
                            "{BRANCH_DUPLICATE}: branch `{}` is recorded with other content",
                            record.branch_id
                        ));
                    }
                    true
                }
            };
            Ok(RunRecordStored {
                key,
                stored: StoredRun::BranchRecord { record },
                replayed,
            })
        }
        // MEDULLA-DECISION-ENGINE-R134.14: every cited ref must read back as cited from
        // the caller's own tenant, else nothing is stored.
        RunRecord::BranchRetrieval { receipt } => {
            if receipt.refs.is_empty() || receipt.consumer_branch_id.trim().is_empty() {
                return Err(format!(
                    "{BRANCH_READBACK_STALE}: a retrieval receipt cites at least one record"
                ));
            }
            for cited in &receipt.refs {
                let Some(bytes) = ctx
                    .store
                    .decision_artifact(ctx.tenant_id, &cited.record_id)?
                else {
                    return Err(format!(
                        "{BRANCH_READBACK_STALE}: `{}` is not stored",
                        cited.record_id
                    ));
                };
                let held: BranchRecord = decode_artifact(&bytes, "branch record")?;
                if BranchRef::of(&held) != *cited
                    || branch_record_key(&held.plan_digest, &held.branch_id) != cited.record_id
                    || held.plan_digest != receipt.plan_digest
                {
                    return Err(format!(
                        "{BRANCH_READBACK_STALE}: `{}` no longer reads back as cited",
                        cited.record_id
                    ));
                }
            }
            let key = branch_retrieval_key(&receipt.plan_digest, &receipt.consumer_branch_id);
            let replayed = ctx.store.decision_artifact(ctx.tenant_id, &key)?.is_some();
            let receipt: BranchRetrievalReceipt = store_once(
                ctx,
                key.clone(),
                "branch retrieval receipt",
                receipt,
                |a, b| a == b,
            )?;
            Ok(RunRecordStored {
                key,
                stored: StoredRun::BranchRetrieval { receipt },
                replayed,
            })
        }
        RunRecord::Insight { insight } => {
            let insight = Insight::from_value(insight).map_err(|error| match error {
                // The registered code leads the text, as every typed refusal does.
                missing @ InsightError::MissingSourceRun => missing.to_string(),
                other => refusal(StatisticalErrorCode::ParameterInvalid, other),
            })?;
            let key = insight_key(&insight);
            let replayed = ctx.store.decision_artifact(ctx.tenant_id, &key)?.is_some();
            let insight = store_once(ctx, key.clone(), "insight", insight, |a, b| a == b)?;
            Ok(RunRecordStored {
                key,
                stored: StoredRun::Insight { insight },
                replayed,
            })
        }
    }
}

/// What the tenant's Blob CAS holds at the receipt's manifest digest. With no
/// receipt there is nothing to read; with no Blob CAS the manifest is
/// unreadable. A read error is surfaced, never read as "absent".
fn read_manifest(
    ctx: &ExecutionContext,
    retention: &Retention,
    receipt: Option<&PrePlanTestReceipt>,
) -> Result<ManifestReading, String> {
    let Some(receipt) = receipt else {
        return Ok(ManifestReading::Unreadable);
    };
    #[cfg(feature = "blob")]
    {
        let Some(blob) = retention.blob.as_ref() else {
            return Ok(ManifestReading::Unreadable);
        };
        let length = medulla_server_blob::blob::engine_bodies::engine_manifest_length(
            blob.store.as_ref(),
            ctx.tenant_id,
            &receipt.artifact_manifest_digest,
        )?;
        Ok(match length {
            None => ManifestReading::Unreadable,
            Some(0) => ManifestReading::Empty,
            Some(_) => ManifestReading::Readable,
        })
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (ctx, retention, receipt);
        Ok(ManifestReading::Unreadable)
    }
}
