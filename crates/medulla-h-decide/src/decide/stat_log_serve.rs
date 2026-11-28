//! The served decision-log entry points, split out above the learning dispatch they call.

use super::stat_log_commit::commit;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_resolve::resolve;
use medulla_h_decide_base::decide::stat_support::refusal;
use medulla_h_decide_base::decide::telemetry;
use medulla_h_decide_base::decide::SharedState;
use medulla_h_decide_learning::decide::stat_replay::replay_view;
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use std::time::Instant;
use tracing::Instrument;
use {
    crate::decide::stat_retention_hi::verify,
    medulla_h_decide_base::decide::stat_retention::compact,
};
use {
    medulla_h_decide_base::decide::stat_log::aggregate_log,
    medulla_h_decide_base::decide::stat_log::breach_link,
    medulla_h_decide_base::decide::stat_log::commit_graphs,
    medulla_h_decide_base::decide::stat_log::commit_run_record,
    medulla_h_decide_base::decide::stat_log::commit_staged_link,
    medulla_h_decide_base::decide::stat_log::evaluate,
    medulla_h_decide_base::decide::stat_log::ReadGraphs,
    medulla_h_decide_base::decide::stat_log::Served,
    medulla_h_decide_learning::decide::stat_log_corrections::correction_labels_of,
};
use {
    medulla_h_decide_base::decide::stat_log_reader::visible_entry,
    medulla_h_decide_base::decide::stat_log_reader::LogReader,
};

pub(super) fn dispatch(
    ctx: &ExecutionContext,
    reader: &LogReader,
    op: DecisionLogOp,
    inputs: Option<&super::stat_learning::LearningInputs>,
    index: &medulla_decision_memory::memory::capability_index::CapabilityIndex,
    read: &ReadGraphs<'_>,
) -> Result<ResultPayload, String> {
    use medulla_protocol_types::{
        result_contract::coordination::DecisionLogAggregate,
        result_contract::coordination::DecisionLogCommit,
        result_contract::coordination::DecisionLogCompact,
        result_contract::coordination::DecisionLogEvaluate,
        result_contract::coordination::DecisionLogGet,
        result_contract::coordination::DecisionLogResolve,
        result_contract::coordination::DecisionLogVerify,
    };
    match op {
        DecisionLogOp::Commit {
            record,
            evaluator,
            approval,
        } => ResultPayload::of::<DecisionLogCommit>(commit(
            ctx,
            reader,
            &record,
            evaluator.as_ref(),
            approval.as_ref(),
            read.replay,
            read.snapshots,
        )?),
        DecisionLogOp::Evaluate { evaluation, .. } => {
            ResultPayload::of::<DecisionLogEvaluate>(evaluate(ctx, reader, evaluation)?)
        }
        DecisionLogOp::Get { record_id, .. } => {
            ResultPayload::of::<DecisionLogGet>(visible_entry(ctx.store, reader, &record_id)?)
        }
        DecisionLogOp::Aggregate { request } => ResultPayload::of::<DecisionLogAggregate>(
            aggregate_log(ctx.store, reader, &request, read.correction_labels)?,
        ),
        DecisionLogOp::Compact { policy, limit, .. } => ResultPayload::of::<DecisionLogCompact>(
            compact(ctx, &reader.retention, &policy, limit)?,
        ),
        DecisionLogOp::Resolve { resolution, .. } => {
            ResultPayload::of::<DecisionLogResolve>(resolve(ctx, reader, resolution)?)
        }
        DecisionLogOp::PromoteRule {
            rule,
            resolution_ids,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogPromoteRule,
        >(
            medulla_h_decide_learning::decide::stat_rule_promotion::promote(
                ctx,
                reader,
                rule,
                &resolution_ids,
            )?,
        ),
        DecisionLogOp::ProposeRule {
            rule,
            scope,
            source_output_digest,
            predecessor,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogProposeRule,
        >(
            medulla_h_decide_learning::decide::stat_rule_promotion::propose(
                ctx,
                rule,
                scope,
                source_output_digest,
                predecessor,
            )?,
        ),
        DecisionLogOp::ApproveRule {
            rule_id,
            version,
            source_output_digest,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogApproveRule,
        >(
            medulla_h_decide_learning::decide::stat_rule_promotion::approve(
                ctx,
                reader,
                &rule_id,
                version,
                &source_output_digest,
            )?,
        ),
        DecisionLogOp::RevokeRule {
            rule_id, version, ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogRevokeRule,
        >(
            medulla_h_decide_learning::decide::stat_rule_promotion::revoke(
                ctx, reader, &rule_id, version,
            )?,
        ),
        DecisionLogOp::PolicyActivation {
            change,
            source,
            receipt_digest,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogPolicyActivation,
        >(
            medulla_h_decide_runlog::decide::policy_activation::activate_policy(
                ctx,
                reader,
                change,
                &source,
                &receipt_digest,
            )?,
        ),
        DecisionLogOp::Learn { write, .. } => {
            ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::DecisionLogLearn,
            >(super::stat_learning::dispatch(ctx, reader, write, inputs)?)
        }
        DecisionLogOp::Writeback {
            tenant_id,
            computation,
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogWriteback,
        >(medulla_h_decide_runlog::decide::stat_writeback::serve(
            ctx,
            reader,
            &tenant_id,
            computation,
        )?),
        DecisionLogOp::FeedbackContinuation { loop_id, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogFeedbackContinuation,
        >(
            medulla_decision_program_types::decision::checker_feedback::ContinuationRead {
                continuation:
                    medulla_h_decide_runlog::decide::family::feedback_loop::latest_continuation(
                        ctx.store, reader, &loop_id,
                    )?,
            },
        ),
        DecisionLogOp::RunRecord { .. } => Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a run record is staged by `run_record::stage` and its graph link is committed \
             through the durable mutation path",
        )),
        DecisionLogOp::CapabilityOutcome {
            entity_id,
            success,
            reward_q32,
            decision_id,
            source_ids,
            policy,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogCapabilityOutcome,
        >(
            medulla_h_decide_base::decide::stat_capability_outcome::apply(
                ctx,
                index,
                &entity_id,
                success,
                reward_q32,
                &decision_id,
                &source_ids,
                &policy,
            )?,
        ),
        DecisionLogOp::GenerationSpace { .. } => {
            Err("generation_space is served on the async path".to_string())
        }
        DecisionLogOp::WritebackAudit {
            tenant_id,
            change_set_id,
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogWritebackAudit,
        >(medulla_h_decide_runlog::decide::stat_writeback::audit_rows(
            ctx,
            reader,
            &tenant_id,
            &change_set_id,
        )?),
        DecisionLogOp::HarnessAdoption { write, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogHarnessAdoption,
        >(
            medulla_h_decide_base::decide::stat_harness_adoption::adopt(ctx, reader, &write)?,
        ),
        DecisionLogOp::Review { review, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogReview,
        >(
            medulla_h_decide_base::decide::stat_review::revision_review(ctx, reader, &review)?,
        ),
        DecisionLogOp::LifecycleRecord { record, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogLifecycleRecord,
        >(
            medulla_h_decide_base::decide::stat_lifecycle_record::write(ctx, record)?,
        ),
        DecisionLogOp::LifecycleRecords { query, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogLifecycleRecords,
        >(
            medulla_h_decide_base::decide::stat_lifecycle_record::read(ctx, &query)?,
        ),
        DecisionLogOp::HarnessLessons { query, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogHarnessLessons,
        >(
            medulla_h_decide_base::decide::stat_harness_adoption::lessons(ctx, &query)?,
        ),
        DecisionLogOp::Pointer { pointer_key, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogPointer,
        >(
            medulla_h_decide_base::decide::stat_pointer::champion_pointer(
                ctx.store,
                ctx.tenant_id,
                &pointer_key,
            )?,
        ),
        DecisionLogOp::List {
            question_kind,
            resolution_kind,
            from_ms,
            to_ms,
            limit,
            cursor,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogList,
        >(medulla_h_decide_runlog::decide::stat_decision_reads::list(
            ctx.store,
            reader,
            &medulla_h_decide_runlog::decide::stat_decision_reads::ListFilter {
                question_kind,
                resolution_kind,
                from_ms,
                to_ms,
                limit,
                cursor,
            },
        )?),
        DecisionLogOp::Provenance { record_id, .. } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogProvenance,
        >(
            medulla_h_decide_runlog::decide::stat_decision_reads::provenance(
                ctx.store,
                reader,
                &record_id,
                read.provenance,
            )?,
        ),
        DecisionLogOp::WhyNot {
            record_id,
            option_id,
            budget_ms,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogWhyNot,
        >(
            medulla_h_decide_runlog::decide::stat_decision_reads::why_not(
                ctx.store,
                ctx.tenant_id,
                &record_id,
                &option_id,
                budget_ms,
            )?,
        ),
        DecisionLogOp::Verify { record_id, .. } => ResultPayload::of::<DecisionLogVerify>(verify(
            ctx,
            reader,
            &reader.retention,
            &record_id,
        )?),
    }
}

pub(super) async fn serve(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: DecisionLogOp,
) -> Result<ResultPayload, String> {
    if op.tenant_id() != verified.tenant() {
        return Err(
            "ACCESS_DENIED: DecisionLog tenant must match the verified request tenant".to_string(),
        );
    }
    let (store, secret) = {
        let mut guard = state.write().await;
        (guard.ensure_agent_library()?, guard.auth_secret.clone())
    };
    if let DecisionLogOp::GenerationSpace { logical, model, .. } = &op {
        let tenant = CarrierAuthority::from_verified(verified)?
            .tenant_scope()
            .to_string();
        let read = medulla_h_decide_runlog::decide::stat_generation_space::serve(
            state,
            &store,
            &tenant,
            logical,
            model.as_ref(),
        )
        .await?;
        return ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogGenerationSpace,
        >(read);
    }
    let reader = LogReader::served(state, verified).await;
    let index = medulla_h_decide_base::decide::stat_designation::shared_capability_index(
        &*state.read().await,
    );
    let inputs = super::stat_learning::prepare(state, verified, &op).await?;
    let run_graph =
        medulla_h_decide_base::decide::run_record::open_graph(state, verified, &op).await?;
    // A committed family answer that re-reads a graph replays over the same
    // filtered view the served `Decide` opened.
    let replay_graph = match &op {
        DecisionLogOp::Commit { record, .. } => replay_view(state, verified, record).await?,
        _ => None,
    };
    let blocking_graph = run_graph.clone();
    let breach = breach_link(state, verified, &op).await?;
    let refresh_link = inputs
        .as_ref()
        .map(|i| std::sync::Arc::clone(&i.refresh_link));
    let refresh_apply = inputs
        .as_ref()
        .map(|i| std::sync::Arc::clone(&i.refresh_apply));
    let principal = verified.agent_id().to_string();
    let graphs = commit_graphs(state, verified, &op).await?;
    let correction_labels = correction_labels_of(state, verified, &op).await?;
    let provenance = match &op {
        DecisionLogOp::Provenance { record_id, .. } => {
            medulla_h_decide_runlog::decide::stat_decision_reads::provenance_graphs(
                state, verified, &store, &reader, record_id,
            )
            .await?
        }
        _ => Vec::new(),
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let ctx = ExecutionContext {
            store: store.as_ref(),
            tenant_id: &reader.tenant_id,
            now_ms,
            server_secret: secret.as_bytes(),
            matched: &[],
            designation: None,
            reader: None,
            assistant_profile: None,
            observation_windows: None,
            type_digest: None,
            recommend_catalog: None,
            defect_triage: None,
        };
        match op {
            DecisionLogOp::RunRecord { record, .. } => {
                medulla_h_decide_base::decide::run_record::stage(
                    &ctx,
                    &reader.retention,
                    record,
                    blocking_graph.as_deref(),
                    &principal,
                    &reader.principal,
                )
                .map(|staged| Served::RunRecord(Box::new(staged)))
            }
            op => dispatch(
                &ctx,
                &reader,
                op,
                inputs.as_ref(),
                &index,
                &ReadGraphs {
                    replay: replay_graph.as_ref(),
                    snapshots: &graphs,
                    correction_labels: &correction_labels,
                    provenance: &provenance,
                },
            )
            .map(Served::Payload),
        }
    })
    .await
    .map_err(|error| format!("DecisionLog task failed: {error}"))?;
    let result = match result {
        Ok(Served::Payload(payload)) => {
            if let Some(link) = breach {
                commit_staged_link(state, req_id, verified, link, "limit-breach").await?;
            }
            let staged = match refresh_link {
                Some(slot) => slot.lock().ok().and_then(|mut held| held.take()),
                None => None,
            };
            let apply =
                refresh_apply.and_then(|slot| slot.lock().ok().and_then(|mut held| held.take()));
            match staged {
                Some(link) => {
                    let committed =
                        commit_staged_link(state, req_id, verified, link, "freshness-refresh")
                            .await;
                    // Durable first: the in-memory swap runs only after the commit succeeded.
                    if committed.is_ok() {
                        if let Some(apply) = apply {
                            apply();
                        }
                    }
                    committed.map(|()| payload)
                }
                None => {
                    if let Some(apply) = apply {
                        apply();
                    }
                    Ok(payload)
                }
            }
        }
        Ok(Served::RunRecord(staged)) => commit_run_record(state, req_id, verified, *staged).await,
        Err(error) => Err(error),
    };
    telemetry::logged(result.is_ok(), started);
    result
}

/// Serve one `DecisionLog` op.
pub(super) async fn handle_log(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: DecisionLogOp,
) -> Response {
    let span = telemetry::span("DecisionLog", verified.tenant());
    match serve(state, req_id, verified, op).instrument(span).await {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => {
            telemetry::refused("DecisionLog", &error);
            Response::err(req_id, error)
        }
    }
}
