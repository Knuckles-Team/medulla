//! The replay entry points, split out above the family registry they call.

use medulla_decision_types::decision::digest::statistical_record_digest;
use medulla_graph_core::graph::GraphView;
use medulla_h_decide_base::decide::stat_classes::current_rules;
use medulla_h_decide_base::decide::stat_support::refusal;
use medulla_h_decide_learning::decide::stat_belief::points;
use {
    medulla_decision_policy_types::decision::statistical::keyed::seed_commitment,
    medulla_decision_policy_types::decision::statistical::keyed::seed_text,
};
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};
use {
    medulla_h_decide_base::decide::stat_executor::recorded_explanation,
    medulla_h_decide_base::decide::stat_executor::run_on_matrix,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::MatrixInputs,
};
use {
    medulla_h_decide_learning::decide::stat_replay::loggable,
    medulla_h_decide_learning::decide::stat_replay::mismatch,
    medulla_h_decide_learning::decide::stat_replay::pinned_and_policy,
    medulla_h_decide_learning::decide::stat_replay::replay_feedback_loop,
    medulla_h_decide_learning::decide::stat_replay::replay_harness_contrast,
    medulla_h_decide_learning::decide::stat_replay::replay_request,
    medulla_h_decide_learning::decide::stat_replay::require_supported_replay,
    medulla_h_decide_learning::decide::stat_replay::same_answer,
    medulla_h_decide_learning::decide::stat_replay::stored_matrix,
};

/// The verify-replay of a `HarnessConfig` record (MEDULLA-DECISION-ENGINE-R128.13.2.1):
/// the family re-derives the configuration from the stored evidence, task
/// facts and pinned policy; the record is logged only when it re-derives.
pub(super) fn replay_harness_config(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<StatisticalDecisionRecord, String> {
    let (_, policy) = pinned_and_policy(ctx, record)?;
    medulla_h_decide_learning::decide::family::harness_config::replay(ctx, record, &policy.policy)?;
    let mut logged = record.clone();
    logged.record_digest = statistical_record_digest(&logged);
    Ok(logged)
}

/// The verify-replay of an advisory answer a question family concluded from
/// its parameters alone (an answer ranking, MEDULLA-DECISION-ENGINE-R134.106;
/// a round verdict, R136.4): the shared ladder cannot reproduce it, so the
/// family's own conclusion re-runs on the stored parameters under the pinned
/// policy and must give the recorded outcome. A kind whose family concludes
/// nothing on its own keeps the refusal of an unlogged advisory score.
pub(super) fn replay_family_conclusion(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
    graph_view: Option<&GraphView>,
) -> Result<StatisticalDecisionRecord, String> {
    let (pinned, policy) = pinned_and_policy(ctx, record)?;
    let request = replay_request(record);
    let family = super::family::registry::for_kind(record.question.kind);
    let Some(concluded) = family.conclude(ctx, &request, &policy.policy, graph_view) else {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "only an acted, explored or abstained decision is logged",
        ));
    };
    let concluded = concluded.map_err(|detail| mismatch(&detail))?;
    let replayed = family.refine(
        None,
        &pinned,
        &[],
        &policy.policy,
        &policy.digest,
        concluded,
    )?;
    let (replayed, _) = medulla_h_decide_base::decide::safety_stage::apply(
        &request,
        &policy.policy,
        record.created_at_ms,
        replayed,
    )?;
    if replayed.ladder.outcome != record.outcome {
        return Err(mismatch(
            "re-running the family conclusion on its stored inputs gives another answer",
        ));
    }
    Ok(record.clone())
}

/// Re-derive `record` and return the copy to log, with its seed revealed.
pub fn replay(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<StatisticalDecisionRecord, String> {
    replay_with_view(ctx, record, None)
}

/// [`replay`] with the graph view a family conclusion re-reads
/// ([`replay_view`]).
pub(super) fn replay_with_view(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
    graph_view: Option<&GraphView>,
) -> Result<StatisticalDecisionRecord, String> {
    if statistical_record_digest(record) != record.record_digest {
        return Err(mismatch("the record digest does not match its content"));
    }
    if record.question.kind == QuestionKind::FeedbackLoop {
        return replay_feedback_loop(ctx, record);
    }
    require_supported_replay(record)?;
    // A sealed `harness_config` record is advisory, but its replay re-derives the
    // configuration from the stored evidence (MEDULLA-DECISION-ENGINE-R128.13.2.1),
    // which the generic family-conclusion replay cannot do: it holds no declared
    // options. It is routed first.
    if record.question.kind == QuestionKind::HarnessConfig {
        return replay_harness_config(ctx, record);
    }
    if matches!(record.outcome, StatisticalOutcome::Advisory { .. }) {
        return replay_family_conclusion(ctx, record, graph_view);
    }
    if !loggable(&record.outcome) {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "only an acted, explored or abstained decision is logged",
        ));
    }
    let inputs = &record.inputs;
    if inputs.harness_contrast.is_some() {
        return replay_harness_contrast(ctx, record);
    }
    if inputs.classification_rules.is_some() && inputs.classification_rules != current_rules() {
        return Err(mismatch(
            "the record's classes were derived under another rule set",
        ));
    }
    let (pinned, policy) = pinned_and_policy(ctx, record)?;
    let matrix_inputs = MatrixInputs {
        question: &record.question,
        params: inputs.params.as_slice(),
        now_ms: inputs.shortlist.now_ms,
        server_secret: ctx.server_secret,
        pool: inputs.exploration.as_ref().and_then(|e| e.pool.as_ref()),
    };
    let matrix = stored_matrix(record)?;
    let options = matrix.candidate_ids.len();
    let replayed = super::family::registry::for_kind(record.question.kind).refine(
        None,
        &pinned,
        &[],
        &policy.policy,
        &policy.digest,
        run_on_matrix(&matrix_inputs, &pinned, &policy, matrix)?,
    )?;
    // The cost tie-break reads the ledger at decide time; its observed costs
    // are sealed premises of the record, which stand in for the ledger here.
    let replayed = if record.question.kind == QuestionKind::Route {
        medulla_h_decide_learning::decide::family::route::replay_cost_tie(
            record,
            replayed,
            &policy.policy,
        )?
    } else {
        replayed
    };
    // The safety bounds withheld or passed the original at its own instant.
    let (replayed, _) = medulla_h_decide_base::decide::safety_stage::apply(
        &replay_request(record),
        &policy.policy,
        record.created_at_ms,
        replayed,
    )?;
    let explanation = recorded_explanation(&pinned, &replayed)? == record.explanation;
    let belief =
        points(&pinned, inputs.belief_slices.as_slice(), options)? == record.belief.as_slice();
    if !same_answer(record, &replayed, explanation && belief) {
        return Err(mismatch(
            "re-running the decision on its stored inputs gives another answer",
        ));
    }
    let mut logged = record.clone();
    if let Some(exploration) = logged.inputs.exploration.as_mut() {
        if exploration.seed_commitment != seed_commitment(&replayed.seed) {
            return Err(mismatch(
                "the exploration seed does not match its commitment",
            ));
        }
        exploration.revealed_seed = Some(seed_text(&replayed.seed));
    }
    logged.record_digest = statistical_record_digest(&logged);
    Ok(logged)
}
