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
use std::sync::Arc;

use medulla_decision_types::decision::program_store::preplan::PrePlanError;
use medulla_decision_types::decision::program_store::{
    compile_program,
    insight::{insight_key, Insight, InsightError},
    preplan::{
        max_receipt_age_ms, pattern_key, verify_preplan, ManifestReading, PrePlanTestReceipt,
        ReusablePattern,
    },
    program_key, program_node_id, CompiledProgram, ProgramError, ProgramKnobs, RunRecord,
    RunRecordStored, StoredRun, COMPILED_FROM, PROGRAM_NODE_CLASS, RUN_TRACE_FIELD,
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
use super::stat_support::{graph_at_level, refusal, resolve_policy};
use super::stat_vectors::GraphVectors;
use super::SharedState;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// The graph node and edge a compiled run commits through the durable path.
pub(super) struct StagedLink {
    pub graph: String,
    pub core: Arc<medulla_graph_core::graph::GraphCore>,
    pub method: Method,
}

/// A run record whose artifacts are stored and whose graph link, when it has
/// one, is still to commit (MEDULLA-DECISION-ENGINE-R135.5).
pub(super) struct Staged {
    pub stored: RunRecordStored,
    pub link: Option<StagedLink>,
}

/// Open the graph a compiled run writes its link into, for the verified
/// caller. `None` for a run record that writes no graph fact.
pub(super) async fn open_graph(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &medulla_decision_types::decision::statistical::log::DecisionLogOp,
) -> Result<Option<Arc<GraphVectors>>, String> {
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    use medulla_decision_types::decision::statistical::StatisticalErrorCode;
    use medulla_isolation::isolation::AccessLevel;
    let DecisionLogOp::RunRecord {
        record: RunRecord::CompiledProgram { source, .. },
        ..
    } = op
    else {
        return Ok(None);
    };
    let (core, isolation) = graph_at_level(
        state,
        verified.agent_id(),
        &source.graph,
        StatisticalErrorCode::ParameterInvalid,
        AccessLevel::Write,
    )
    .await?;
    Ok(Some(Arc::new(GraphVectors::from_parts(
        core,
        isolation,
        verified.agent_id().to_string(),
    ))))
}

fn program_refusal(error: ProgramError) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, error)
}

/// The source node must be a `RunTrace` receipt node the caller can see, whose
/// record names the run the program compiles. A node that is absent and a node
/// the caller may not see refuse the same way.
fn check_trace_node(
    view: &GraphVectors,
    graph: &str,
    node_id: &str,
    run_id: &str,
) -> Result<(), ProgramError> {
    let unknown = || ProgramError::TraceUnknown {
        graph: graph.to_string(),
        node_id: node_id.to_string(),
    };
    let properties = view.visible_properties(node_id).ok_or_else(unknown)?;
    if properties.get("kind").and_then(|kind| kind.as_str()) != Some("run_trace") {
        return Err(unknown());
    }
    let named = properties
        .get(RUN_TRACE_FIELD)
        .and_then(|record| record.get("run_id"))
        .and_then(|id| id.as_str());
    if named != Some(run_id) {
        return Err(ProgramError::Invalid(format!(
            "trace node `{node_id}` records another run than `{run_id}`"
        )));
    }
    Ok(())
}

/// The `BatchUpdate` that writes the program node, typed with its ontology
/// class, and its `compiled_from` edge. Upserts, so a retry of a commit whose
/// graph write failed converges.
fn link_batch(
    node_id: &str,
    program: &CompiledProgram,
    trace_node: &str,
    tenant: &str,
    principal: &str,
) -> Result<Method, String> {
    let operations = serde_json::json!([
        {
            "op": "upsert_node",
            "id": node_id,
            "properties": {
                "type": PROGRAM_NODE_CLASS,
                "task_signature": program.task_signature,
                "program_digest": program.program_digest,
                "source_run_id": program.source_run_id,
                "step_count": program.steps.len(),
                "_owner": principal,
            },
        },
        {
            "op": "upsert_edge",
            "source": node_id,
            "target": trace_node,
            "properties": {"relationship": COMPILED_FROM, "tenant": tenant},
        },
    ]);
    let operations_msgpack = rmp_serde::to_vec_named(&operations).map_err(|error| {
        refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("run record link encoding failed: {error}"),
        )
    })?;
    Ok(Method::BatchUpdate { operations_msgpack })
}

/// Store one run record's artifacts and stage its graph link. A refused
/// compile stores nothing; the link of a compiled run commits after the
/// artifacts, through the durable mutation path (`stat_log`).
pub(super) fn stage(
    ctx: &ExecutionContext,
    retention: &Retention,
    record: RunRecord,
    view: Option<&GraphVectors>,
    principal: &str,
) -> Result<Staged, String> {
    let source = match &record {
        RunRecord::CompiledProgram { source, .. } => Some(source.clone()),
        _ => None,
    };
    let stored = commit(ctx, retention, record, view)?;
    let link = match (&stored.stored, source, view) {
        (
            StoredRun::CompiledProgram {
                program,
                program_node,
                ..
            },
            Some(source),
            Some(view),
        ) => {
            let method = link_batch(
                program_node,
                program,
                &source.node_id,
                ctx.tenant_id,
                principal,
            )?;
            Some(StagedLink {
                graph: source.graph,
                core: Arc::clone(view.core()),
                method,
            })
        }
        _ => None,
    };
    Ok(Staged { stored, link })
}

/// Commit one run record. A refused compile stores nothing; a retry with equal
/// content answers the stored row and other content under the key refuses
/// `IDEMPOTENCY_CONFLICT`.
fn commit(
    ctx: &ExecutionContext,
    retention: &Retention,
    record: RunRecord,
    view: Option<&GraphVectors>,
) -> Result<RunRecordStored, String> {
    match record {
        RunRecord::CompiledProgram {
            trace,
            policy,
            source,
            reusable,
            plan_binding,
            preplan_receipt,
        } => {
            let resolved = resolve_policy(ctx.store, ctx.tenant_id, &policy)?;
            let knobs = ProgramKnobs::from_policy(&resolved.policy);
            let program = compile_program(&trace, &knobs).map_err(program_refusal)?;
            // MEDULLA-DECISION-ENGINE-R135.5: the source must be a run trace the
            // caller sees, checked before anything is stored.
            let view = view.ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    "a compiled run is staged against its opened graph",
                )
            })?;
            check_trace_node(view, &source.graph, &source.node_id, &trace.run_id)
                .map_err(program_refusal)?;
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
            let program_node = program_node_id(&program.task_signature);
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
                stored: StoredRun::CompiledProgram {
                    program,
                    program_node,
                    pattern,
                },
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
