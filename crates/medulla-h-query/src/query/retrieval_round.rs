//! `Method::RetrievalRound` (MEDULLA-FEDERATED-QUERY-R081.11): one retrieval round.
//!
//! The handler binds the plan exactly as `UnifiedQuery` does: the caller's
//! row-level-security snapshot, narrowed to the caller's tenant by the one read
//! chokepoint, with the caller's verified legs. `medulla_plan::retrieval_round::run_round`
//! then executes the plan and the requested parts inside that bound context. The
//! method writes nothing and caches nothing.

use super::planning::{run_unified_off_lock_with, UnifiedRead};
use super::*;
use medulla_plan::retrieval_round::{run_round, RoundError, RoundKnobs};
use medulla_protocol_types::result_contract::query::RetrievalRound;
use medulla_result_types::retrieval_round::{RoundRequest, RoundResult};

/// The registered knobs one round reads.
fn round_knobs() -> RoundKnobs {
    let knobs = medulla_server_api::knob_config::knobs();
    RoundKnobs {
        min_evidence: knobs.retrieval_min_evidence,
        round_limit: u32::try_from(knobs.retrieval_round_limit).unwrap_or(u32::MAX),
    }
}

pub(crate) async fn handle_retrieval_round(
    ctx: &QueryHandlerCtx<'_>,
    request: RoundRequest,
) -> Response {
    let req_id = ctx.req_id;
    let core = ctx.core.clone();
    let legs = match ctx
        .served_legs_with_foreign_executions(&request.plan, None, &[])
        .await
    {
        Ok(legs) => legs,
        Err(refusal) => return refusal,
    };
    let (snap, _version) = sql_read_snapshot(
        &core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let plan = request.plan.clone();
    let knobs = round_knobs();
    let read = UnifiedRead { snap, plan, legs };
    let outcome: Result<Result<Result<RoundResult, RoundError>, String>, Response> =
        run_unified_off_lock_with(
            ctx.state,
            req_id,
            &core,
            read,
            &scope,
            move |_plan, plan_ctx| Ok(run_round(&request, plan_ctx, &knobs)),
        )
        .await;
    match outcome {
        Err(response) => response,
        Ok(Err(message)) => Response::err(req_id, message),
        Ok(Ok(Err(RoundError::Plan(message)))) => Response::err(req_id, message),
        Ok(Ok(Err(RoundError::Refusal(refusal)))) => {
            Response::err(req_id, format!("{}: {}", refusal.code, refusal.detail))
        }
        Ok(Ok(Ok(result))) => match ResultPayload::of::<RetrievalRound>(result) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, format!("INTERNAL: {error}")),
        },
    }
}
