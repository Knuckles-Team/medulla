//! `Method::RetrievalRound` (MEDULLA-FEDERATED-QUERY-R081.11): one retrieval round.
//!
//! The handler binds the plan exactly as `UnifiedQuery` does: the caller's
//! row-level-security snapshot, narrowed to the caller's tenant by the one read
//! chokepoint, with the caller's verified legs. `medulla_plan::retrieval_round::run_round`
//! then executes the plan and the requested parts inside that bound context. The
//! method writes nothing and caches nothing.

use super::planning::{run_unified_off_lock_with, UnifiedRead};
use super::*;
use medulla_exec::exec::{LexicalOverlapReranker, Reranker, LEXICAL_OVERLAP_RERANKER};
use medulla_plan::retrieval_round::{run_round, RoundError, RoundKnobs};
use medulla_protocol_types::result_contract::query::RetrievalRound;
use medulla_result_types::retrieval_round::{RoundRequest, RoundResult};

/// The registered knobs one round reads.
fn round_knobs() -> RoundKnobs {
    let knobs = medulla_server_api::knob_config::knobs();
    let size = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
    RoundKnobs {
        min_evidence: knobs.retrieval_min_evidence,
        round_limit: u32::try_from(knobs.retrieval_round_limit).unwrap_or(u32::MAX),
        rerank_pool: size(knobs.retrieval_rerank_pool),
        selection_quality_floor: knobs.retrieval_selection_quality_floor,
        selection_cost_ratio_max: knobs.retrieval_selection_cost_ratio_max,
        section_max_depth: size(knobs.retrieval_section_max_depth),
        section_max_visited: size(knobs.retrieval_section_max_visited),
    }
}

/// The process-wide reranker named by the server config key `retrieval.reranker_model`
/// (environment `MEDULLA_RETRIEVAL_RERANKER_MODEL`), or `None` when it is unset
/// (MEDULLA-FEDERATED-QUERY-R081.7.3). `builtin:lexical-overlap` binds the deterministic
/// [`LexicalOverlapReranker`]; any other value names a model artifact and binds nothing
/// until that cross-encoder provider is registered, so a selection request refuses
/// `RETRIEVAL_SCORE_PAIR_REQUIRED` rather than scoring with a stand-in.
fn bound_reranker() -> Option<&'static dyn Reranker> {
    use std::sync::OnceLock;
    static RERANKER: OnceLock<Option<Box<dyn Reranker>>> = OnceLock::new();
    RERANKER
        .get_or_init(|| {
            match std::env::var("MEDULLA_RETRIEVAL_RERANKER_MODEL")
                .ok()
                .as_deref()
            {
                Some(LEXICAL_OVERLAP_RERANKER) => {
                    Some(Box::new(LexicalOverlapReranker) as Box<dyn Reranker>)
                }
                _ => None,
            }
        })
        .as_deref()
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
            move |_plan, plan_ctx| {
                // The embedder the stored summaries share a space with and the bound
                // reranker ride on this round's own copy of the context.
                let mut round_ctx = plan_ctx.clone();
                if round_ctx.embedder.is_none() {
                    if let Some(embedder) = uql_text_embedder() {
                        round_ctx = round_ctx.with_embedder(embedder);
                    }
                }
                if let Some(reranker) = bound_reranker() {
                    round_ctx = round_ctx.with_reranker(reranker);
                }
                Ok(run_round(&request, &round_ctx, &knobs))
            },
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
