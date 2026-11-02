//! `Method::Decide`: the statistical executor.
//!
//! Served by [`super::stat_decide`] in a build with the `decide` feature; a
//! build without it answers a typed refusal naming the feature.

use super::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Answer a batch of statistical decision records; commit none.
pub async fn handle_decide(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_decision_types::decision::DecideRequest,
) -> Response {
    #[cfg(feature = "decide")]
    {
        super::stat_decide::handle_decide(state, req_id, verified, request).await
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, verified, request);
        Response::err(req_id, "Decide requires the `decide` feature")
    }
}
