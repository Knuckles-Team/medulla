//! The correction labels of the decision log, split out above the corrections they read.

use medulla_h_decide_base::decide::SharedState;
use medulla_server_api::authority_context::VerifiedRequestContext;
use {
    medulla_decision_types::decision::statistical::log::CorrectionLabel,
    medulla_decision_types::decision::statistical::log::DecisionLogOp,
};

/// The independent correction outcomes an aggregate request asks to join (MEDULLA-DECISION-ENGINE-R059):
/// read once, through the one read of `CorrectionOp::Outcomes` (R702.8), from the named graph
/// as the caller sees it. A request that asks for none joins none.
pub async fn correction_labels_of(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &DecisionLogOp,
) -> Result<Vec<CorrectionLabel>, String> {
    let DecisionLogOp::Aggregate { request } = op else {
        return Ok(Vec::new());
    };
    let Some(asked) = &request.corrections else {
        return Ok(Vec::new());
    };
    let (view, _) = medulla_h_decide_base::decide::stat_support::filtered_view_versioned(
        state,
        verified.agent_id(),
        &asked.graph,
    )
    .await?;
    Ok(
        super::correction::independent_outcomes(&view, verified.tenant(), asked.rule_id.as_deref())
            .into_iter()
            .map(|candidate| CorrectionLabel {
                rule_id: candidate.rule_id,
                task_class: candidate.task_class,
                independent: candidate.independent,
                mean_value_micros: (candidate.mean_value.clamp(0.0, 1.0) * 1_000_000.0).round()
                    as u64,
            })
            .collect(),
    )
}
