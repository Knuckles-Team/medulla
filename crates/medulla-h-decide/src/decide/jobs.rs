//! `Method::DecisionFit` and `Method::DecisionEval`: the two admin jobs behind
//! a calibrated decision head.
//!
//! Served by [`super::stat_jobs`] in a build with the `decide` feature; a
//! build without it answers a typed refusal naming the feature.

use super::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Submit or read one head-fitting job.
pub async fn handle_decision_fit(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: medulla_decision_types::decision::DecisionFitOp,
) -> Response {
    #[cfg(feature = "decide")]
    {
        super::stat_jobs::handle_fit(state, req_id, verified, op).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "DecisionFit requires the `decide` feature")
    }
}

/// Submit or read one head-evaluation job.
pub async fn handle_decision_eval(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: medulla_decision_types::decision::DecisionEvalOp,
) -> Response {
    #[cfg(feature = "decide")]
    {
        super::stat_jobs::handle_eval(state, req_id, verified, op).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "DecisionEval requires the `decide` feature")
    }
}

/// Plan the `skill_eval` job of `DecisionEval` with its graph: the suite report node is the batch
/// the router commits (MEDULLA-DECISION-ENGINE-R711.3, MEDULLA-ORCH-VOCAB-R817.4).
pub async fn handle_skill_eval(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
    request: medulla_decision_types::skill_eval::SkillEvalRequest,
) -> Result<super::planned_write::PlannedWrite, String> {
    #[cfg(feature = "decide")]
    {
        super::stat_jobs::plan_skill_eval(state, verified, graph, request).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, graph, request);
        Err("DecisionEval requires the `decide` feature".to_string())
    }
}
