//! The decision-log commit, split out above the replay it verifies with.

use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_log_reader::LogReader;
use medulla_h_decide_base::decide::stat_support::refusal;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::record_key;
use {
    crate::decide::stat_replay_hi::replay_with_view,
    medulla_h_decide_base::decide::stat_replay_lo::GraphSnapshots,
    medulla_h_decide_learning::decide::stat_replay::verify_graph_rows,
};
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
};
use {
    medulla_decision_types::decision::statistical::log::DecisionLogCommitted,
    medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    medulla_decision_types::decision::statistical::log::EntryInputs,
    medulla_decision_types::decision::statistical::log::DECISION_LOG_SCHEMA_VERSION,
};
use {
    medulla_h_decide_base::decide::stat_log::skill_program_rows,
    medulla_h_decide_base::decide::stat_log::visibility_of,
};

pub(super) fn commit(
    ctx: &ExecutionContext,
    reader: &LogReader,
    record: &StatisticalDecisionRecord,
    evaluator: Option<&medulla_decision_types::decision::statistical::log::NamedEvaluator>,
    approval: Option<&medulla_decision_policy_types::decision::approval::RecommendationApproval>,
    replay_graph: Option<&medulla_graph_core::graph::GraphView>,
    graphs: &GraphSnapshots,
) -> Result<DecisionLogCommitted, String> {
    if record.caller_principal != reader.principal {
        return Err(
            "ACCESS_DENIED: a decision is logged by the principal that made it".to_string(),
        );
    }
    // MEDULLA-DECISION-ENGINE-R019.100.2: the receipt names the digest the reviewer saw (the
    // submitted record's, before the seed reveal) and is checked before the idempotency read,
    // so a refusal writes nothing and a retry carries its approval again.
    let resolved = medulla_h_decide_base::decide::stat_support::resolve_policy(
        ctx.store,
        ctx.tenant_id,
        &record.inputs.policy,
    )?;
    if resolved.policy.approval_required {
        medulla_decision_policy_types::decision::approval::check_approval(
            approval,
            &record.record_id,
            &record.record_digest,
            ctx.now_ms,
            medulla_decision_policy_types::decision::approval::approval_max_age_s(&resolved.policy),
        )
        .map_err(|refusal| refusal.message())?;
    }
    let logged = match medulla_h_decide_learning::decide::stat_replay::verify_disclosure_event(
        ctx, reader, record,
    )? {
        Some(verified) => verified,
        None => replay_with_view(ctx, record, replay_graph)?,
    };
    let key = record_key(&logged.record_id);
    let committed = |replayed| DecisionLogCommitted {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        record_id: logged.record_id.clone(),
        record_digest: logged.record_digest.clone(),
        replayed,
    };
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let existing: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
        if existing.record.record_digest == logged.record_digest {
            return Ok(committed(true));
        }
        return Err(refusal(
            StatisticalErrorCode::IdempotencyConflict,
            "the record id is logged with other content",
        ));
    }
    // A new record's graph rows are re-read now; an already logged record
    // answers above without it, so a repeat commit stays idempotent after the
    // graph moves on.
    verify_graph_rows(record, graphs)?;
    let entry = DecisionLogEntry {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        visibility: visibility_of(&logged, &reader.principal),
        record: Box::new(logged.clone()),
        committed_by: reader.principal.clone(),
        committed_at_ms: ctx.now_ms,
        inputs: EntryInputs::Inline,
        approval: approval.cloned(),
    };
    // EH-395: a named evaluator's grant is written in the same transaction.
    let mut rows = medulla_h_decide_base::decide::stat_evaluator::grant_rows(
        ctx,
        &logged.record_id,
        &reader.principal,
        evaluator,
    )?;
    rows.extend(skill_program_rows(ctx, &logged)?);
    rows.push((key, encode_artifact(&entry)?));
    ctx.store.put_decision_artifacts(ctx.tenant_id, &rows)?;
    Ok(committed(false))
}
