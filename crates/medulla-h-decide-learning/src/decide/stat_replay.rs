//! Verify-replay of a statistical record before it is logged (§4.4, EH-060).
//!
//! A record arriving on the wire is never trusted. Its digest is re-checked,
//! its pinned feature schema, head and policy are re-read by pin, and the
//! decision function re-runs on the STORED feature matrix, parameters and
//! clock -- never on live features, which move with every write. The outcome,
//! calibration statement, audit draw, explanation, logging distribution and
//! belief slices (EH-297, from their stored matrices) must come out
//! identical, and the recomputed keyed seed must match the record's
//! commitment; only then is the seed revealed in the logged copy.

use medulla_decision_statistical_types::decision::DecisionErrorCode;
use medulla_decision_stats::decision::features::FeatureMatrix;
use medulla_decision_types::decision::digest::statistical_record_digest;
use medulla_decision_types::decision::statistical::harness_config::HARNESS_REPLAY_MISMATCH;
use medulla_decision_types::{
    decision::harness_contrast::replay_record, decision::harness_contrast::ContrastKnobs,
    decision::harness_contrast::CONTRAST_FEATURE_NAMES,
};
use medulla_graph_core::graph::GraphView;
use medulla_h_decide_base::decide::stat_replay_lo::GraphSnapshots;
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::FeatureMatrixRef,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};
use {
    medulla_h_decide_base::decide::stat_executor::pinned_inputs,
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::Pinned,
};
use {
    medulla_h_decide_base::decide::stat_support::materialize_matrix,
    medulla_h_decide_base::decide::stat_support::refusal,
    medulla_h_decide_base::decide::stat_support::resolve_policy,
    medulla_h_decide_base::decide::stat_support::ResolvedPolicy,
};

pub fn mismatch(detail: &str) -> String {
    format!(
        "{}: {detail}",
        DecisionErrorCode::DecisionReplayMismatch.as_str()
    )
}

pub fn stored_matrix(record: &StatisticalDecisionRecord) -> Result<FeatureMatrix, String> {
    let FeatureMatrixRef::Inline {
        candidate_ids,
        feature_names,
        values,
        ..
    } = &record.inputs.feature_matrix
    else {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "only an inline feature matrix can be replayed",
        ));
    };
    if values.is_empty() {
        return Err(mismatch("the record stores no feature values to replay"));
    }
    Ok(materialize_matrix(
        candidate_ids.as_slice(),
        feature_names.as_slice(),
        values.as_slice(),
    ))
}

/// Re-read the graph rows a record names AT the version each was read at and
/// refuse a difference (MEDULLA-DECISION-ENGINE-R121.2). The view for a row is
/// built from the graph's retained node history, so a graph that moved on after
/// the decision still replays, and a row changed or removed afterwards does not
/// make the replay differ; a row whose image at that version differs from the
/// record, or that the caller cannot see in that image, is refused.
pub fn verify_graph_rows(
    record: &StatisticalDecisionRecord,
    graphs: &GraphSnapshots,
) -> Result<(), String> {
    for row in record.inputs.graph_rows.iter() {
        let view = graphs
            .get(&(row.graph.clone(), row.graph_version))
            .ok_or_else(|| {
                mismatch(&format!(
                    "graph {} was not re-read at version {} for row {}",
                    row.graph, row.graph_version, row.node_id
                ))
            })?;
        let properties = view
            .node_properties
            .get(&row.node_id)
            .and_then(|blob| medulla_types::msgpack::decode_property_value(blob.as_slice()).ok())
            .ok_or_else(|| {
                mismatch(&format!(
                    "graph row {} was not visible in {} at version {}",
                    row.node_id, row.graph, row.graph_version
                ))
            })?;
        let then = medulla_h_decide_base::decide::candidates::graph_row_provenance(
            &row.graph,
            row.graph_version,
            &row.node_id,
            &properties,
        )
        .map_err(|detail| mismatch(&detail))?;
        if then != *row {
            return Err(mismatch(&format!(
                "graph row {} did not have the recorded class and source at version {}",
                row.node_id, row.graph_version
            )));
        }
    }
    Ok(())
}

/// An executed decision is logged so an outcome can evaluate it; an
/// abstention is logged so its escalation can be resolved against it
/// (EH-037). An advisory score executed nothing and resolves nothing.
pub fn loggable(outcome: &StatisticalOutcome) -> bool {
    matches!(
        outcome,
        StatisticalOutcome::Acted { .. }
            | StatisticalOutcome::Explored { .. }
            | StatisticalOutcome::Abstained { .. }
    )
}

/// Verify a record whose answer is a disclosure event before it is logged
/// (MEDULLA-DECISION-ENGINE-R143.3). The decision is evaluate-only and is not
/// re-run on a stored matrix; the log commit is its compare-and-set: the digest
/// must hold, the event and its sealed premise must agree, and the event must
/// be exactly the one the caller's own log makes next (version 1 for the
/// initial disclosure, the latest version plus one for a refinement with a
/// changed hash). A record already logged answers idempotently.
pub fn verify_disclosure_event(
    ctx: &ExecutionContext,
    reader: &medulla_h_decide_base::decide::stat_log_reader::LogReader,
    record: &StatisticalDecisionRecord,
) -> Result<Option<StatisticalDecisionRecord>, String> {
    use medulla_decision_types::decision::instruction_disclosure::{scan_limit, DisclosureTrigger};
    let Some(event) = record
        .answer
        .as_ref()
        .and_then(|answer| answer.disclosure_event())
    else {
        return Ok(None);
    };
    if statistical_record_digest(record) != record.record_digest {
        return Err(mismatch("the record digest does not match its content"));
    }
    if record.question.kind != QuestionKind::InstructionDisclosure {
        return Err(mismatch(
            "a disclosure event rides only an instruction_disclosure question",
        ));
    }
    event
        .validate()
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    let fact = super::family::instruction_disclosure::event_premise_fact(event);
    if !record.premises.iter().any(|premise| premise.fact == fact) {
        return Err(mismatch(
            "the record states no premise for its disclosure event",
        ));
    }
    let key = medulla_persistence_records::decision_jobs::record_key(&record.record_id);
    if ctx.store.decision_artifact(ctx.tenant_id, &key)?.is_some() {
        return Ok(Some(record.clone()));
    }
    let (_, policy) = pinned_and_policy(ctx, record)?;
    let prior = medulla_h_decide_base::decide::stat_log::latest_disclosure(
        ctx.store,
        reader,
        &event.set_id,
        scan_limit(&policy.policy),
    )?;
    let (expected, code) = match (&prior, event.trigger) {
        (None, DisclosureTrigger::Initial) => (1, StatisticalErrorCode::DisclosureVersionStale),
        (None, _) => {
            return Err(refusal(
                StatisticalErrorCode::DisclosurePriorMissing,
                "a refinement needs the prior disclosure event",
            ))
        }
        (Some(_), DisclosureTrigger::Initial) => {
            return Err(refusal(
                StatisticalErrorCode::DisclosurePriorUnexpected,
                "the initial disclosure of this set is already logged",
            ))
        }
        (Some(prior), _) => {
            if prior.instructions_hash == event.instructions_hash {
                return Err(refusal(
                    StatisticalErrorCode::DisclosureHashUnchanged,
                    "the refinement's instructions hash equals the prior hash",
                ));
            }
            (
                prior.version.saturating_add(1),
                StatisticalErrorCode::DisclosureVersionStale,
            )
        }
    };
    if event.version != expected {
        return Err(refusal(
            code,
            format!(
                "event version {} is not the next version {expected} of set {}",
                event.version, event.set_id
            ),
        ));
    }
    Ok(Some(record.clone()))
}

/// `derived` says the explanation and the belief slices re-derived equal.
pub fn same_answer(record: &StatisticalDecisionRecord, replayed: &Executed, derived: bool) -> bool {
    record.outcome == replayed.ladder.outcome
        && record.calibration == replayed.ladder.calibration
        && record.audit == replayed.ladder.audit
        && record.logging_propensities.as_slice() == replayed.ladder.logging.as_slice()
        && derived
}

pub fn require_supported_replay(record: &StatisticalDecisionRecord) -> Result<(), String> {
    // A harness_config record carries its evidence as its answer; it replays
    // through its own re-derivation, never through the score replay.
    if (record.answer.is_some() && record.question.kind != QuestionKind::HarnessConfig)
        || record.question.kind == medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind::TaskPlan
    {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "task plans are evaluate-only and cannot use statistical score replay",
        ));
    }
    if record.inputs.harness_evaluation.is_some()
        && record.question.kind != QuestionKind::HarnessConfig
    {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!(
                "{HARNESS_REPLAY_MISMATCH}: harness evidence rides only a harness_config question"
            ),
        ));
    }
    if record.inputs.harness_contrast.is_some()
        && record.question.kind != QuestionKind::HarnessContrast
    {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a harness contrast decision cannot ride another question kind",
        ));
    }
    Ok(())
}

/// The verify-replay of a `HarnessContrast` record (MEDULLA-DECISION-ENGINE-R803.5):
/// the generic ladder re-run cannot reproduce the contrast outcome, so the
/// stored inputs are re-derived by the decision's own deterministic core —
/// its pins still resolve, its stored matrix carries exactly the recorded
/// candidates and deltas, and re-running the adoption rule over the stored
/// deltas gives the recorded outcome. The graph evidence (held versions,
/// probe runs, stored contrasts) was read at decide time and is not
/// replayable; the replay verifies the core and the digest, the same trust
/// model as the generic replay over a stored matrix.
pub fn replay_harness_contrast(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<StatisticalDecisionRecord, String> {
    let inputs = &record.inputs;
    let (pinned, policy) = pinned_and_policy(ctx, record)?;
    let mut names = pinned.schema.names();
    names.sort();
    let expected: Vec<&str> = CONTRAST_FEATURE_NAMES.to_vec();
    if names.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err(mismatch(
            "the pinned feature schema is not the contrast delta schema",
        ));
    }
    if inputs.head.is_some() {
        return Err(mismatch("a contrast decision pins no head"));
    }
    replay_record(record, &ContrastKnobs::from_policy(&policy.policy))
        .map_err(|detail| mismatch(&detail))?;
    let mut logged = record.clone();
    logged.record_digest = statistical_record_digest(&logged);
    Ok(logged)
}

/// The record's pinned feature schema and head, and its pinned policy, which
/// must still carry the recorded digest.
pub fn pinned_and_policy(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<(Pinned, ResolvedPolicy), String> {
    let inputs = &record.inputs;
    let pinned = pinned_inputs(ctx, &inputs.feature_schema, inputs.head.as_ref())?;
    let policy = resolve_policy(ctx.store, ctx.tenant_id, &inputs.policy)?;
    if policy.digest != inputs.policy_digest {
        return Err(mismatch(
            "the pinned policy no longer has the recorded digest",
        ));
    }
    Ok((pinned, policy))
}

/// The verify-replay of a `FeedbackLoop` record (MEDULLA-DECISION-ENGINE-R169.3):
/// the pinned policy still carries the recorded digest, and the continue-or-stop
/// rule re-run over the sealed round and bound gives the recorded outcome. The
/// record carries no answer: the round is its own evidence.
pub fn replay_feedback_loop(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<StatisticalDecisionRecord, String> {
    if record.answer.is_some() {
        return Err(mismatch("a feedback round carries no family answer"));
    }
    let (_, policy) = pinned_and_policy(ctx, record)?;
    let outcome = medulla_h_decide_runlog::decide::family::feedback_loop::round_outcome(
        record.inputs.params.as_slice(),
        &policy.policy,
    )
    .map_err(|detail| mismatch(&detail))?;
    if outcome != record.outcome {
        return Err(mismatch(
            "re-running the round on its stored inputs gives another answer",
        ));
    }
    Ok(record.clone())
}

/// The request a logged record was decided from, as far as its stored inputs
/// carry it: the question, pins, policy and parameters. Declared options are
/// stored only by digest, so none are rebuilt.
pub fn replay_request(
    record: &StatisticalDecisionRecord,
) -> medulla_decision_types::decision::statistical::DecideRequest {
    use medulla_decision_types::decision::statistical::{CandidateSource, DecideRequest};
    use medulla_types::contract::BoundedVec;
    let inputs = &record.inputs;
    DecideRequest {
        tenant_id: record.tenant_id.clone(),
        question: record.question.clone(),
        candidates: CandidateSource::Declared {
            options: BoundedVec::default(),
        },
        feature_schema: inputs.feature_schema.clone(),
        head: inputs.head.clone(),
        policy: inputs.policy.clone(),
        params: inputs.params.clone(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: inputs.evidence_streams.clone(),
    }
}

/// The graph view a family conclusion re-reads on replay, opened the way the
/// served `Decide` opened it for the same parameters and caller (RLS-filtered):
/// a `fanout_aggregate` answer checks its stored readback node in the named
/// graph (MEDULLA-DECISION-ENGINE-R134.101). `None` for every record whose
/// replay reads no graph.
pub async fn replay_view(
    state: &medulla_h_decide_base::decide::SharedState,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    record: &StatisticalDecisionRecord,
) -> Result<Option<GraphView>, String> {
    // A `merge_arming` answer re-reads the delivery, verdicts and approvals it concluded from.
    if matches!(record.outcome, StatisticalOutcome::Advisory { .. })
        && record.question.kind == QuestionKind::MergeArming
    {
        return super::family::merge_arming::arming_view(
            state,
            verified.agent_id(),
            &replay_request(record),
        )
        .await
        .map(Some);
    }
    if !matches!(record.outcome, StatisticalOutcome::Advisory { .. })
        || record.question.kind != QuestionKind::FanoutAggregate
    {
        return Ok(None);
    }
    medulla_h_decide_runlog::decide::family::fanout_aggregate::fanout_view(
        state,
        verified.agent_id(),
        verified.tenant(),
        &replay_request(record),
    )
    .await
    .map(Some)
}
