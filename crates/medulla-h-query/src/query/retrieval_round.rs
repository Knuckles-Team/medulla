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
use medulla_plan::doc_state::{segment_policy, HashSegmentsEncoder, StateSource};
use medulla_plan::retrieval_round::{run_round_with_state, RoundError, RoundKnobs, StateRuntime};
use medulla_protocol_types::result_contract::query::RetrievalRound;
use medulla_result_types::retrieval_round::{RoundRequest, RoundResult};
use medulla_result_types::retrieval_verdict::RETRIEVAL_BUDGET_EXCEEDED;
use medulla_types::retrieval_round::DocumentState;

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
        token_limit: knobs.retrieval_token_limit,
        inference_limit: u32::try_from(knobs.retrieval_inference_limit).unwrap_or(u32::MAX),
        cost_limit: knobs.retrieval_cost_limit,
        saturation_epsilon: knobs.retrieval_saturation_epsilon,
        joint_lambda: knobs.retrieval_joint_lambda,
        joint_tolerance: knobs.retrieval_joint_tolerance,
        joint_max_iters: u32::try_from(knobs.retrieval_joint_max_iters).unwrap_or(u32::MAX),
        feedback_queries: u32::try_from(knobs.retrieval_feedback_queries).unwrap_or(u32::MAX),
        feedback_terms: u32::try_from(knobs.retrieval_feedback_terms).unwrap_or(u32::MAX),
    }
}

/// What the server config key `retrieval.reranker_model` (environment
/// `MEDULLA_RETRIEVAL_RERANKER_MODEL`) binds, read once per process.
pub(super) enum RerankerBinding {
    /// The key is unset.
    Unset,
    /// The key names a model that is loaded.
    Bound(Box<dyn Reranker>),
    /// The key names a model artifact no provider is registered for.
    Unavailable,
}

pub(super) fn reranker_binding() -> &'static RerankerBinding {
    use std::sync::OnceLock;
    static RERANKER: OnceLock<RerankerBinding> = OnceLock::new();
    RERANKER.get_or_init(|| {
        match std::env::var("MEDULLA_RETRIEVAL_RERANKER_MODEL")
            .ok()
            .as_deref()
        {
            None | Some("") => RerankerBinding::Unset,
            Some(LEXICAL_OVERLAP_RERANKER) => {
                RerankerBinding::Bound(Box::new(LexicalOverlapReranker))
            }
            Some(_) => RerankerBinding::Unavailable,
        }
    })
}

/// The process-wide reranker named by the server config key
/// `retrieval.reranker_model`, or `None` when it is unset or names an artifact
/// with no registered provider (MEDULLA-FEDERATED-QUERY-R081.7.3).
/// `builtin:lexical-overlap` binds the deterministic [`LexicalOverlapReranker`];
/// any other value names a model artifact and binds nothing until that
/// cross-encoder provider is registered, so a selection request refuses
/// `RETRIEVAL_SCORE_PAIR_REQUIRED` rather than scoring with a stand-in.
fn bound_reranker() -> Option<&'static dyn Reranker> {
    match reranker_binding() {
        RerankerBinding::Bound(reranker) => Some(reranker.as_ref()),
        _ => None,
    }
}

/// Reads stored states from the shared durable store.
struct StoreSource(std::sync::Arc<medulla_persistence::doc_state_store::DocumentStateStore>);

impl StateSource for StoreSource {
    fn get(
        &self,
        document_id: &str,
        content_digest: &str,
        encoder_revision: &str,
        policy_revision: &str,
    ) -> Result<Option<DocumentState>, String> {
        self.0.get(
            document_id,
            content_digest,
            encoder_revision,
            policy_revision,
        )
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
    // The state part reads the shared store with the bound encoder; a build is never run here.
    let state_source = if request.state.is_some()
        && medulla_server_api::knob_config::knobs().retrieval_state_builtin_encoder
    {
        let guard = ctx.state.read().await;
        medulla_persistence::doc_state_store::shared_doc_state_store(&guard)
            .ok()
            .map(StoreSource)
    } else {
        None
    };
    let policy_revision = segment_policy(
        usize::try_from(medulla_server_api::knob_config::knobs().retrieval_state_segment_bytes)
            .unwrap_or(usize::MAX),
    );
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
                let runtime = state_source.as_ref().map(|source| StateRuntime {
                    source,
                    encoder: &HashSegmentsEncoder,
                    policy_revision: policy_revision.clone(),
                });
                Ok(run_round_with_state(
                    &request,
                    &round_ctx,
                    &knobs,
                    runtime.as_ref(),
                ))
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
        // MEDULLA-FEDERATED-QUERY-R082.4: an exhausted limit answers with the budget code
        // and the terminal verdict (reason, round, exhausted dimension, calibration marker).
        Ok(Ok(Ok(result)))
            if result
                .stop
                .as_ref()
                .is_some_and(|stop| stop.exhausted_dimension.is_some()) =>
        {
            let verdict = serde_json::to_string(&result.stop).unwrap_or_default();
            Response::err(req_id, format!("{RETRIEVAL_BUDGET_EXCEEDED}: {verdict}"))
        }
        Ok(Ok(Ok(result))) => match ResultPayload::of::<RetrievalRound>(result) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, format!("INTERNAL: {error}")),
        },
    }
}
