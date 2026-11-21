//! `Method::Decide`: the statistical executor, served (EH-056, EH-059).
//!
//! Evaluate-only: it reads the visible candidates, the pinned feature schema,
//! head and policy, runs the ladder and answers a `DecisionBatch` of one
//! sealed record. It commits nothing, so the exploration seed's reveal is
//! left to the commit that makes the record durable.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::Instrument;

use medulla_decision_types::decision::digest::statistical_record_digest;
use medulla_decision_types::decision::statistical::keyed::seed_commitment;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::{
    decision::statistical::nl::NlBinding, decision::statistical::nl::NlChoiceSource,
};
use medulla_decision_types::{
    decision::statistical::CandidatePool, decision::statistical::CandidateSource,
    decision::statistical::DecideRequest, decision::statistical::DecisionBatch,
    decision::statistical::ExplorationRecord, decision::statistical::FeatureMatrixRef,
    decision::statistical::QuestionKind, decision::statistical::ShortlistProvenance,
    decision::statistical::StatisticalDecisionRecord, decision::statistical::StatisticalErrorCode,
    decision::statistical::StatisticalInputs, decision::statistical::StatisticalOutcome,
    decision::statistical::declared::MAX_DECLARED_OPTIONS,
};
use medulla_decision_types::{
    decision::EvidenceClass, decision::PremiseClass, decision::PremiseProvenance,
    decision::PremiseRef, decision::QuantScaleTag, decision::ResolutionKind,
    decision::TraceFidelity, decision::STATISTICAL_DECISION_RECORD_SCHEMA_VERSION,
};
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::contract::BoundedVec;

use super::candidates::{read_candidates, ReadCandidates};
use super::stat_belief::{live_slices, Slices};
use super::stat_executor::{
    execute, pinned_inputs, recorded_explanation, Executed, ExecutionContext, Pinned,
};
use super::stat_log::{fill_observed_costs, fill_outcome_rates, LogReader};
use super::stat_nl::binding;
use super::stat_support::{refusal, resolve_policy, ResolvedPolicy};
use super::telemetry;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Domain of a statistical record's input digest.
pub const STATISTICAL_INPUTS_DOMAIN: &str = "eg/decision-statistical-inputs/v1";

/// Answer one statistical decision; commit nothing.
pub async fn handle_decide(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: DecideRequest,
) -> Response {
    let span = telemetry::span("Decide", verified.tenant());
    let result = serve(state, verified, request).instrument(span).await;
    match result
        .and_then(ResultPayload::of::<medulla_protocol_types::result_contract::query::Decide>)
    {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => {
            telemetry::refused("Decide", &error);
            Response::err(req_id, error)
        }
    }
}

/// The `ConflictDecider` seam (MEDULLA-DECISION-ENGINE-R828): the `BranchMerge`
/// handler of the graph handler crate decides one conflict through it. It runs
/// the same served path as [`handle_decide`] under the caller's verified
/// context -- admission, `decide_blocking`, `execute` and `family::for_kind`
/// -- for a `resolve_conflict` question only, and returns the sealed record.
pub struct ConflictDeciderSeam;

impl medulla_server_api::api::conflict_decider::ConflictDecider for ConflictDeciderSeam {
    fn decide_conflict<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        verified: &'a VerifiedRequestContext,
        request: DecideRequest,
    ) -> medulla_server_api::api::reentry::SeamFuture<'a, Result<StatisticalDecisionRecord, String>>
    {
        Box::pin(async move {
            if request.question.kind != QuestionKind::ResolveConflict {
                return Err(refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    "the conflict decider answers resolve_conflict questions only",
                ));
            }
            let span = telemetry::span("Decide", verified.tenant());
            let batch = serve(state, verified, request).instrument(span).await?;
            batch.records.as_slice().first().cloned().ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    "a conflict decision returned no record",
                )
            })
        })
    }
}

async fn serve(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    request: DecideRequest,
) -> Result<DecisionBatch, String> {
    let family = telemetry::family(request.question.kind);
    telemetry::stage(telemetry::Stage::Admit, &family, verified.tenant(), || {
        if request.tenant_id != verified.tenant() {
            return Err(
                "ACCESS_DENIED: Decide tenant must match the verified request tenant".to_string(),
            );
        }
        medulla_decision_types::decision::evidence_stream::validate(
            request.evidence_streams.as_slice(),
        )
        .map_err(|refused| refused.message())?;
        if request.max_records == Some(0) {
            return Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "max_records must be at least 1",
            ));
        }
        Ok(())
    })?;
    let (store, secret) = {
        let mut guard = state.write().await;
        (guard.ensure_agent_library()?, guard.auth_secret.clone())
    };
    // A `topology` question reads its schema entailments and capacity headroom
    // here, where the graph and the capacity cells are reachable, and records
    // them in its own parameter (MEDULLA-DECISION-ENGINE-R111.1).
    // A `capability_designation` question loads the resident index handle and
    // the schema closure it concludes over (MEDULLA-DECISION-ENGINE-R172.7).
    let designation = match request.question.kind {
        QuestionKind::CapabilityDesignation => Some(
            telemetry::stage_async(
                telemetry::Stage::Entailment,
                &family,
                verified.tenant(),
                super::stat_designation::with_runtime(state, verified, &request),
            )
            .await?,
        ),
        // A `load_set` question reads the same index and schema closure, plus
        // the local agents of its graph (MEDULLA-DECISION-ENGINE-R176.3).
        QuestionKind::LoadSet => Some(
            telemetry::stage_async(
                telemetry::Stage::Entailment,
                &family,
                verified.tenant(),
                super::stat_load_set::with_load_set_runtime(state, verified, &request),
            )
            .await?,
        ),
        // A `usage_guidance` question reads the connector pack of its graph
        // and the tenant's tool outcomes (MEDULLA-DECISION-ENGINE-R177.3).
        QuestionKind::UsageGuidance => Some(
            telemetry::stage_async(
                telemetry::Stage::Entailment,
                &family,
                verified.tenant(),
                super::stat_usage_guidance::with_usage_guidance_runtime(
                    state, &store, verified, &request,
                ),
            )
            .await?,
        ),
        _ => None,
    };
    let request = if request.question.kind == QuestionKind::Topology {
        telemetry::stage_async(
            telemetry::Stage::Entailment,
            &family,
            verified.tenant(),
            super::stat_topology::with_read(state, store.as_ref(), verified, request),
        )
        .await?
    } else if request.question.kind == QuestionKind::PaperAdmission {
        // A `paper_admission` question reads the nearest-neighbour similarities
        // and the graph version here, where the graph's vectors are reachable,
        // and records them in its own parameters (MEDULLA-DECISION-ENGINE-R031.100).
        telemetry::stage_async(
            telemetry::Stage::Visibility,
            &family,
            verified.tenant(),
            super::family::with_similarity(state, store.as_ref(), verified, request),
        )
        .await?
    } else if request.question.kind == QuestionKind::ResolveEntity {
        // A `resolve_entity` question grows its candidate pairs from the
        // tenant's own embedded neighbours and reads the reviews of earlier
        // proposals here, where the graph and the log are reachable
        // (MEDULLA-DECISION-ENGINE-R706.3).
        telemetry::stage_async(
            telemetry::Stage::Candidates,
            &family,
            verified.tenant(),
            super::family::with_candidates(state, store.as_ref(), verified, request),
        )
        .await?
    } else if request.question.kind == QuestionKind::CapacityProposal {
        // A `capacity_proposal` question reads the cell's headroom and epoch
        // from the capacity ledger here and records them in its own parameter
        // (MEDULLA-DECISION-ENGINE-R824).
        telemetry::stage_async(
            telemetry::Stage::Entailment,
            &family,
            verified.tenant(),
            super::stat_capacity::with_read(state, request),
        )
        .await?
    } else if request.question.kind == QuestionKind::SkillEvolution {
        // A `skill_evolution` question reads the stored embedding of each
        // visible skill here, through the caller's graph access, and records
        // the vectors in its own parameter (MEDULLA-DECISION-ENGINE-R703.3).
        telemetry::stage_async(
            telemetry::Stage::Visibility,
            &family,
            verified.tenant(),
            super::family::with_skill_embeddings(state, store.as_ref(), verified, request),
        )
        .await?
    } else {
        request
    };
    // A `placement` question reads the declared facts of its targets and the
    // capacity headroom of its cells here, and records them in its parameter
    // (MEDULLA-PLACEMENT-R001.7).
    let request = if request.question.kind == QuestionKind::Placement {
        telemetry::stage_async(
            telemetry::Stage::Entailment,
            &family,
            verified.tenant(),
            super::stat_placement::with_read(state, verified, request),
        )
        .await?
    } else {
        request
    };
    // A `placement_plan` question reads the `placement:Host` budgets of its
    // graph here, under the caller's access, and records them in its parameter
    // (MEDULLA-PLACEMENT-R700.3).
    let request = if request.question.kind == QuestionKind::PlacementPlan {
        telemetry::stage_async(
            telemetry::Stage::Entailment,
            &family,
            verified.tenant(),
            super::stat_placement_plan::with_read(state, verified, request),
        )
        .await?
    } else {
        request
    };
    // A `harness_contrast`, `recursion_mode` or `sub_call_admission` question
    // declares its candidates but still reads its evidence (held versions,
    // probe runs, stored contrasts, signal references, stored signatures) from
    // the graph named by its `graph` param, under the same RLS filter.
    let graph_view = telemetry::stage_async(
        telemetry::Stage::Visibility,
        &family,
        verified.tenant(),
        async {
            if matches!(
                request.question.kind,
                QuestionKind::HarnessContrast
                    | QuestionKind::RecursionMode
                    | QuestionKind::SubCallAdmission
            ) {
                super::stat_harness_contrast::contrast_evidence_view(
                    state,
                    verified.agent_id(),
                    &request,
                )
                .await
                .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::EditConcentration {
                super::stat_concentration::concentration_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::HeldItemAdmission {
                super::family::held_item_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::CommandConfig {
                super::family::command_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::GraphIntake {
                // A `graph_intake` question resolves its sources in the graph named
                // by its `source_graph` param, under the same ACL and RLS filter
                // (MEDULLA-DECISION-ENGINE-R163.3).
                let graph = super::family::graph_intake::source_graph(&request)?;
                super::stat_support::filtered_view(state, verified.agent_id(), graph)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::FanoutAggregate {
                super::family::fanout_view(state, verified.agent_id(), verified.tenant(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::RuntimeQuota {
                super::family::quota_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| view.map(|view| (view, 0)))
            } else if request.question.kind == QuestionKind::InstructionDisclosure {
                super::family::disclosure_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| view.map(|view| (view, 0)))
            } else if request.question.kind == QuestionKind::ContextPackage {
                // A `context_package` question declares its candidates and checks
                // each against its cited source node in the graph named by its
                // `source_graph` param, under the same ACL and RLS filter.
                let graph = super::family::context_package::source_graph(&request)?;
                super::stat_support::filtered_view(state, verified.agent_id(), graph)
                    .await
                    .map(|view| Some((view, 0)))
            } else if super::family::planning_admission::reads_repository_graph(&request) {
                super::family::planning_admission::planning_view(
                    state,
                    verified.agent_id(),
                    &request,
                )
                .await
                .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::TeamCoordination {
                super::stat_team::team_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::ToolActionAdmission {
                super::family::admission_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::AnalystSynthesis {
                // An `analyst_synthesis` question resolves each citation against the
                // graph named by its `source_graph` param, under the same ACL and RLS filter.
                let graph = super::family::context_package::source_graph(&request)?;
                super::stat_support::filtered_view(state, verified.agent_id(), graph)
                    .await
                    .map(|view| Some((view, 0)))
            } else if request.question.kind == QuestionKind::HarnessConfig {
                super::family::harness_config::evidence_view(state, verified.agent_id(), &request)
                    .await
                    .map(|view| view.map(|view| (view, 0)))
            } else {
                super::candidates::filtered_graph_view(
                    state,
                    verified.agent_id(),
                    &request.candidates,
                )
                .await
            }
        },
    )
    .await?;
    let (graph_view, graph_version) = match graph_view {
        Some((view, version)) => (Some(view), version),
        None => (None, 0),
    };
    // A `harness_config` question that names an assistant profile reads it
    // here, under the visibility rule of the profile store, before any
    // decision input is read (MEDULLA-DECISION-ENGINE-R128.16).
    let assistant_profile = telemetry::stage_async(
        telemetry::Stage::Visibility,
        &family,
        verified.tenant(),
        async {
            if request.question.kind == QuestionKind::HarnessConfig {
                super::family::harness_config::resolve_profile(state, verified, &request).await
            } else {
                Ok(None)
            }
        },
    )
    .await?;
    let principal = verified.principal_persistence_id();
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let started = Instant::now();
    // The blocking pool thread has neither this task's subscriber nor its
    // current span: carry both so the stage spans nest under the method span.
    let parent = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(|current| current.clone());
    tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || {
            parent.in_scope(|| {
                let reader = LogReader {
                    tenant_id: request.tenant_id.clone(),
                    principal: principal.clone(),
                    roles: Vec::new(),
                    retention: super::stat_retention::Retention::none(),
                };
                let ctx = ExecutionContext {
                    store: store.as_ref(),
                    tenant_id: &request.tenant_id,
                    now_ms,
                    server_secret: secret.as_bytes(),
                    matched: &[],
                    designation: designation.as_ref(),
                    reader: Some(&reader),
                    assistant_profile: assistant_profile.as_ref(),
                };
                decide_blocking(
                    &ctx,
                    &request,
                    graph_view.as_ref(),
                    graph_version,
                    principal,
                    started,
                )
            })
        })
    })
    .await
    .map_err(|error| format!("Decide task failed: {error}"))?
}

pub(super) fn decide_blocking(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
    graph_version: u64,
    principal: String,
    started: Instant,
) -> Result<DecisionBatch, String> {
    use telemetry::Stage;
    let family = telemetry::family(request.question.kind);
    let (family, tenant) = (family.as_str(), ctx.tenant_id);
    let registry = super::family::for_kind(request.question.kind);
    let task_plan = super::stat_task_plan::plan_of_request(ctx, request, &principal)?;
    let mut candidates = telemetry::stage(Stage::Candidates, family, tenant, || {
        // A tool_relevance question reads its own index (R134.110.3), so the
        // stage-2 candidate scan and its cap do not apply to it.
        if request.question.kind == QuestionKind::ToolRelevance {
            return super::candidates::indexed_tool_candidates(&request.candidates);
        }
        read_candidates(
            ctx.store,
            ctx.tenant_id,
            &request.candidates,
            graph_view,
            graph_version,
            &principal,
            registry,
            request.params.as_slice(),
        )
    })?;
    let (pinned, policy, provider_binding) =
        telemetry::stage(Stage::Constraints, family, tenant, || {
            registry.constraints(request)?;
            let provider_binding = registry.bind(ctx, request)?;
            let pinned = pinned_inputs(ctx, &request.feature_schema, request.head.as_ref())?;
            let policy = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
            Ok::<_, String>((pinned, policy, provider_binding))
        })?;
    let comparative = super::family::comparative_analysis::plan(
        request,
        &candidates.entries,
        ctx.tenant_id,
        &policy.policy,
        &policy.digest,
    )?;
    let reader = LogReader {
        tenant_id: ctx.tenant_id.to_string(),
        principal: principal.clone(),
        roles: Vec::new(),
        retention: super::stat_retention::Retention::none(),
    };
    // A trajectory query rides on the plan: its facts are the policy knobs, the
    // clock and the outcome records this reader may see (R140.104).
    let (task_plan, trajectory_premises) = match task_plan {
        Some(super::stat_task_plan::Planned::Plan(mut plan, notes)) => {
            match super::stat_trajectory::query_of(request) {
                Some(query) => {
                    let (decision, premises) =
                        super::stat_trajectory::decide(ctx, &reader, &policy, query)?;
                    plan.trajectory = Some(decision);
                    (
                        Some(super::stat_task_plan::Planned::Plan(plan, notes)),
                        premises,
                    )
                }
                None => (
                    Some(super::stat_task_plan::Planned::Plan(plan, notes)),
                    Vec::new(),
                ),
            }
        }
        other => (other, Vec::new()),
    };
    let context_digest = super::stat_log::context_digest_of(
        &request.question.question_id,
        request.params.as_slice(),
    );
    let matched = matched_history(ctx, &reader, &policy, &context_digest)?;
    tracing::debug!(
        matched = matched.len(),
        "decision context matched prior records"
    );
    let ctx = &ExecutionContext {
        matched: &matched,
        reader: Some(&reader),
        ..*ctx
    };
    // The context-matched pool gates exploration (R020.1.2): options executed
    // in a matched record are exploited, and a non-empty exploitation pool
    // restricts ranking to it. Only a decision that may explore has a pool.
    let pool = candidate_pool(
        request,
        &policy,
        &context_digest,
        &candidates.views,
        &matched,
    );
    if let Some(pool) = pool.as_ref().filter(|p| !p.exploitation.is_empty()) {
        candidates
            .views
            .retain(|v| pool.exploitation.contains(&v.id));
        candidates
            .entries
            .retain(|e| pool.exploitation.contains(&e.component_id));
    }
    fill_outcome_rates(
        ctx.store,
        &reader,
        &pinned.schema,
        policy.statistical.tenant_public_features,
        &mut candidates.views,
    )?;
    // A rank catalog larger than one record holds is ranked whole and sealed
    // as its best 64; the Advisory reports how many were ranked
    // (MEDULLA-DECISION-ENGINE-R829.1).
    let ranked_total = candidates.views.len();
    if matches!(request.candidates, CandidateSource::RankCatalog { .. })
        && ranked_total > MAX_DECLARED_OPTIONS
    {
        let best = super::stat_executor::best_candidate_ids(
            ctx,
            request,
            &pinned,
            &candidates.views,
            MAX_DECLARED_OPTIONS,
        )?;
        candidates.views.retain(|v| best.contains(&v.id));
    }
    if request.question.kind == QuestionKind::Route {
        fill_observed_costs(ctx.store, ctx.tenant_id, &mut candidates.views)?;
    }
    // The kind's family concludes its specialized answer; the shared
    // matrix ladder is the fall-through (R001.101).
    let mut executed = telemetry::stage(Stage::Solve, family, tenant, || {
        // A rank question that names a distilled scorer ranks from stored
        // scores alone (R147.3): no head reading, so nothing to refine.
        if let Some(ranked) =
            super::stat_distilled::rank(ctx, request, &policy.policy, &candidates.views)?
        {
            return Ok(ranked);
        }
        let executed =
            match registry.conclude_with_log(ctx, &reader, request, &policy.policy, graph_view) {
                Some(result) => result?,
                None => execute(
                    ctx,
                    request,
                    &pinned,
                    &policy,
                    &candidates.views,
                    pool.as_ref(),
                )?,
            };
        registry.refine(
            Some(request),
            &pinned,
            &candidates.views,
            &policy.policy,
            &policy.digest,
            executed,
        )
    })?;
    if let StatisticalOutcome::Advisory {
        ranked_total: total,
        ..
    } = &mut executed.ladder.outcome
    {
        *total = (*total).max(ranked_total as u32);
    }
    // The policy's safety bounds are checked after the decision concludes and
    // before it is sealed (MEDULLA-DECISION-ENGINE-R001.100.2).
    let (executed, safety_premises) =
        super::safety_stage::apply(request, &policy.policy, ctx.now_ms, executed)?;
    // The policy's limits are checked against the proposed action next
    // (MEDULLA-DECISION-ENGINE-R131.3).
    let (executed, limit_premises) = super::limit_stage::apply(request, &policy.policy, executed)?;
    let mut trajectory_premises = trajectory_premises;
    trajectory_premises.extend(safety_premises);
    trajectory_premises.extend(limit_premises);
    // Admitted retrieved entries enter as premises (MEDULLA-DECISION-ENGINE-R132.3).
    trajectory_premises.extend(super::entry_stage::apply(request, &policy.policy)?);
    let slices = telemetry::stage(Stage::Entailment, family, tenant, || {
        live_slices(ctx, request, &pinned, &candidates.views, graph_view)
    })?;
    let nl = telemetry::stage(Stage::Rank, family, tenant, || {
        binding(request, &executed.ladder.outcome, &candidates.entries)
    })?;
    let record = telemetry::stage(Stage::Record, family, tenant, || {
        seal(
            Sealing {
                ctx,
                request,
                pinned: &pinned,
                policy: &policy,
                candidates: &candidates,
                principal,
                slices: &slices,
                registry,
                context_digest,
                pool,
                binding: provider_binding,
            },
            &executed,
            nl,
            task_plan,
            comparative,
            trajectory_premises,
        )
    })?;
    let batch = telemetry::stage(Stage::Conclude, family, tenant, || {
        telemetry::decided(&record, candidates.views.len(), started);
        let records = BoundedVec::new(vec![record])
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
        Ok(DecisionBatch {
            schema_version: STATISTICAL_DECISION_RECORD_SCHEMA_VERSION,
            inputs_digest: records.as_slice()[0].inputs_digest.clone(),
            records,
        })
    })
    .map_err(|detail| registry.refusals(detail))?;
    Ok(batch)
}

/// Everything sealing reads besides the executed answer.
struct Sealing<'a> {
    ctx: &'a ExecutionContext<'a>,
    request: &'a DecideRequest,
    pinned: &'a Pinned,
    policy: &'a ResolvedPolicy,
    candidates: &'a ReadCandidates,
    principal: String,
    slices: &'a Slices,
    registry: &'a dyn super::family::DecisionFamily,
    context_digest: String,
    /// The context-matched candidate pool; `Some` exactly when exploration is
    /// permitted for this question (MEDULLA-DECISION-ENGINE-R020.1.2).
    pool: Option<CandidatePool>,
    /// The provider a retrieval-plan decision committed to (R029.1).
    binding: Option<
        medulla_decision_types::decision::statistical::retrieval_plan::RetrievalProviderBinding,
    >,
}

/// The pool a decision that may explore draws against. A refused permit
/// yields no pool: the executor reports the refusal.
fn candidate_pool(
    request: &DecideRequest,
    policy: &ResolvedPolicy,
    context_digest: &str,
    views: &[medulla_decision_stats::decision::candidate::CandidateView],
    matched: &[super::stat_log::MatchedTrajectory],
) -> Option<CandidatePool> {
    use medulla_decision_stats::decision::exploration::{build_pool, permit, ExplorationPermit};
    if !matches!(
        permit(&policy.policy, &request.question),
        Ok(ExplorationPermit::Budget(_))
    ) {
        return None;
    }
    let visible: Vec<String> = views.iter().map(|v| v.id.clone()).collect();
    let pairs: Vec<(String, String)> = matched
        .iter()
        .map(|m| (m.record_id.clone(), m.option_id.clone()))
        .collect();
    Some(build_pool(context_digest, &visible, &pairs))
}

/// The same-principal records that share this decision's context, within the
/// policy's `exploration.*` knobs (MEDULLA-DECISION-ENGINE-R020.1.1).
fn matched_history(
    ctx: &ExecutionContext,
    reader: &LogReader,
    policy: &ResolvedPolicy,
    context_digest: &str,
) -> Result<Vec<super::stat_log::MatchedTrajectory>, String> {
    use medulla_decision_stats::decision::exploration::KNOBS;
    use medulla_decision_types::decision::knobs::KnobValue;
    let (KnobValue::DurationMs(window_ms), KnobValue::Integer(max_matched)) =
        (policy.policy.knob(&KNOBS[0]), policy.policy.knob(&KNOBS[1]))
    else {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "exploration knobs have the wrong value type",
        ));
    };
    super::stat_log::matched_trajectories(
        ctx.store,
        reader,
        context_digest,
        super::stat_log::MatchWindow {
            now_ms: ctx.now_ms,
            window_ms,
            max_matched: usize::try_from(max_matched).unwrap_or(0),
        },
    )
}

fn bounded<T, const N: usize>(values: Vec<T>) -> Result<BoundedVec<T, N>, String> {
    BoundedVec::new(values)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))
}

fn feature_matrix(
    views: &[medulla_decision_stats::decision::candidate::CandidateView],
    pinned: &Pinned,
    executed: &Executed,
) -> Result<FeatureMatrixRef, String> {
    let (ids, values) = match &executed.matrix {
        Some(matrix) => (matrix.candidate_ids.clone(), matrix.values.clone()),
        None => (views.iter().map(|v| v.id.clone()).collect(), Vec::new()),
    };
    Ok(FeatureMatrixRef::Inline {
        candidate_ids: bounded(ids)?,
        feature_names: bounded(pinned.schema.names())?,
        scale: QuantScaleTag::Q32,
        values: bounded(values)?,
    })
}

fn exploration(sealing: &Sealing, executed: &Executed) -> Option<ExplorationRecord> {
    if matches!(
        sealing.request.question.kind,
        QuestionKind::TaskPlan
            | QuestionKind::ThoughtSearch
            | QuestionKind::RewardDecomposition
            | QuestionKind::PopulationDrift
            | QuestionKind::FanoutWidth
            | QuestionKind::TrajectoryJoin
            | QuestionKind::StepVerification
            | QuestionKind::AdversarialAdmission
    ) {
        return None; // Explicit planning, search and the kernel families never draw.
    }
    let medulla_decision_types::decision::ColdStart::Explore { budget } =
        &sealing.policy.policy.cold_start
    else {
        return None;
    };
    Some(ExplorationRecord {
        seed_commitment: seed_commitment(&executed.seed),
        revealed_seed: None,
        budget_digest: digest_text("eg/decide-exploration-budget/v1", budget),
        pool: sealing.pool.clone(),
    })
}

fn premise(
    subject: &str,
    fact: &str,
    class: PremiseClass,
    provenance: PremiseProvenance,
) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact: fact.to_string(),
        class,
        provenance,
    }
}

/// One CLAIM premise per rule-derived class (EH-200): a derivation over a
/// publisher's declared facts is only as strong as those facts.
fn derived_premises<'a>(sealing: &'a Sealing) -> impl Iterator<Item = PremiseRef> + 'a {
    sealing.candidates.entries.iter().flat_map(move |entry| {
        let classes = sealing
            .candidates
            .derived
            .by_component
            .get(&entry.component_id);
        classes.into_iter().flatten().map(move |(class, rule)| {
            premise(
                &entry.component_id,
                &format!("derived:{class} by {rule}"),
                PremiseClass::Claim,
                PremiseProvenance::Publisher {
                    component_id: entry.component_id.clone(),
                    definition_digest: entry.definition_digest.clone(),
                },
            )
        })
    })
}

/// One CLAIM premise per caller-declared option: the caller vouches for its
/// facts, nobody else does.
fn declared_premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Vec::new();
    };
    options
        .iter()
        .map(|option| {
            premise(
                &option.option_id,
                "declared_option",
                PremiseClass::Claim,
                PremiseProvenance::Request {
                    field: "candidates.options".to_string(),
                },
            )
        })
        .collect()
}

/// One premise per graph row read, `graph_row:<node_id>`, of the class the
/// row's typed label gives it (MEDULLA-DECISION-ENGINE-R121.2). The record's
/// evidence class is the weakest premise class, so a claim row can never
/// lift an answer to an observation.
fn graph_row_premises(candidates: &ReadCandidates) -> Vec<PremiseRef> {
    use medulla_decision_types::decision::statistical::graph_rows::GraphNodeClass;
    candidates
        .graph_rows
        .iter()
        .map(|row| {
            premise(
                &row.node_id,
                &format!("graph_row:{}", row.node_id),
                match row.node_class {
                    GraphNodeClass::Observation => PremiseClass::Observation,
                    GraphNodeClass::Claim => PremiseClass::Claim,
                },
                PremiseProvenance::GraphRow {
                    graph: row.graph.clone(),
                    graph_version: row.graph_version,
                },
            )
        })
        .collect()
}

/// One DEFINITION premise per knob this decision read, `knob:<name>=<value>`,
/// bound to the digest of the policy that supplied the value
/// (MEDULLA-DECISION-ENGINE-R179.5). The decision reads the drift thresholds
/// of the belief slices it consulted through `DriftThresholds::from_policy`.
fn knob_premises(sealing: &Sealing) -> Vec<PremiseRef> {
    let contrast: &[medulla_decision_types::decision::knobs::KnobSpec] = match sealing
        .request
        .question
        .kind
    {
        QuestionKind::HarnessContrast => medulla_decision_types::decision::harness_contrast::KNOBS,
        QuestionKind::ThoughtSearch => medulla_decision_types::decision::thought_search::KNOBS,
        QuestionKind::RoundVerdict => {
            medulla_decision_types::decision::knob_families::round_verdict::KNOBS
        }
        QuestionKind::Route => medulla_decision_types::decision::knob_families::route::KNOBS,
        QuestionKind::ToolNecessity => medulla_decision_types::decision::tool_necessity::KNOBS,
        QuestionKind::ItemRecommendation => {
            medulla_decision_types::decision::knob_families::recommend::KNOBS
        }
        QuestionKind::ResolveEntity => {
            medulla_decision_types::decision::knob_families::resolve_entity::KNOBS
        }
        QuestionKind::StageAllocation => medulla_decision_types::decision::harness_contrast::KNOBS,
        QuestionKind::ContextPackage | QuestionKind::RecursionMode => {
            medulla_decision_types::decision::knob_families::context::KNOBS
        }
        QuestionKind::OverWindowPlan => {
            medulla_decision_types::decision::knob_families::context::KNOBS
        }
        // A task plan of any proposer (explicit, decomposition, skill query)
        // lists the whole family table (MEDULLA-DECISION-ENGINE-R134.7); the
        // skill-tree knobs are a subset of it.
        QuestionKind::TemporalBasis => medulla_decision_types::decision::temporal_query::KNOBS,
        QuestionKind::RuntimeQuota => medulla_decision_types::decision::run_governance::KNOBS,
        QuestionKind::ProgramLookup => medulla_decision_types::decision::program_store::KNOBS,
        QuestionKind::InsightSelection => {
            medulla_decision_types::decision::program_store::insight::KNOBS
        }
        QuestionKind::TaskPlan => medulla_decision_types::decision::knob_families::task_plan::KNOBS,
        QuestionKind::ToolRelevance => medulla_decision_types::decision::tool_relevance::KNOBS,
        QuestionKind::ComparativeAnalysisPlan => {
            medulla_decision_types::decision::comparative_analysis::KNOBS
        }
        QuestionKind::ProbeVerdict | QuestionKind::ProbeSelection => {
            medulla_decision_types::decision::knob_families::red_team::KNOBS
        }
        QuestionKind::TrajectoryLadder => {
            medulla_decision_types::decision::knob_families::trajectory::KNOBS
        }
        QuestionKind::AnalystSynthesis => {
            medulla_decision_types::decision::analyst_synthesis::KNOBS
        }
        QuestionKind::ResolveConflict => medulla_decision_types::decision::resolve_conflict::KNOBS,
        QuestionKind::CriterionReview => {
            medulla_decision_types::decision::knob_families::criterion_review::KNOBS
        }
        QuestionKind::DelegationAdmission => medulla_decision_types::decision::delegation::KNOBS,
        QuestionKind::RunAdjustment => medulla_decision_types::decision::run_adjustment::KNOBS,
        QuestionKind::PathPreference => medulla_decision_types::decision::path_success::KNOBS,
        QuestionKind::TeamCoordination => {
            medulla_decision_types::decision::team_coordination::KNOBS
        }
        QuestionKind::ChangesetReview => medulla_decision_stats::decision::changeset_review::KNOBS,
        QuestionKind::RewardDecomposition => medulla_decision_types::decision::reward::KNOBS,
        QuestionKind::CurriculumPromotion => medulla_decision_types::decision::curriculum::KNOBS,
        QuestionKind::AutonomyVerdict => medulla_decision_types::decision::autonomy::KNOBS,
        QuestionKind::PopulationDrift | QuestionKind::FanoutWidth => {
            medulla_decision_types::decision::population::KNOBS
        }
        QuestionKind::StepVerification | QuestionKind::AdversarialAdmission => {
            medulla_decision_types::decision::trajectory_verdict::KNOBS
        }
        QuestionKind::HarnessConfig => {
            medulla_decision_types::decision::statistical::harness_config::HARNESS_KNOBS
        }
        _ => &[],
    };
    medulla_decision_stats::decision::drift::KNOBS
        .iter()
        .chain(contrast)
        .map(|spec| {
            premise(
                "policy",
                &format!(
                    "knob:{}={}",
                    spec.name,
                    sealing.policy.policy.knob(spec).render()
                ),
                PremiseClass::Definition,
                PremiseProvenance::Policy {
                    policy_digest: sealing.policy.digest.clone(),
                },
            )
        })
        .collect()
}

fn premises(sealing: &Sealing, nl: Option<&NlBinding>) -> Vec<PremiseRef> {
    let schema = &sealing.request.feature_schema;
    let mut out = vec![
        premise(
            &sealing.context_digest,
            "context_digest",
            PremiseClass::Definition,
            PremiseProvenance::Request {
                field: "params".to_string(),
            },
        ),
        premise(
            &schema.component_id,
            "feature_schema",
            PremiseClass::Definition,
            PremiseProvenance::Publisher {
                component_id: schema.component_id.clone(),
                definition_digest: schema.definition_digest.clone(),
            },
        ),
        premise(
            "policy",
            "decision_policy",
            PremiseClass::Definition,
            PremiseProvenance::Policy {
                policy_digest: sealing.policy.digest.clone(),
            },
        ),
    ];
    if let (Some(pin), Some((head, _))) = (&sealing.request.head, &sealing.pinned.head) {
        let class = if head.synthetic {
            PremiseClass::Claim
        } else {
            PremiseClass::Observation
        };
        out.push(premise(
            &pin.component_id,
            "decision_head",
            class,
            PremiseProvenance::Publisher {
                component_id: pin.component_id.clone(),
                definition_digest: pin.definition_digest.clone(),
            },
        ));
    }
    if let Some(pool) = &sealing.pool {
        out.extend(pool.matched_records.iter().map(|id| {
            premise(
                id,
                &format!("matched_record:{id}"),
                PremiseClass::Observation,
                PremiseProvenance::Observation {
                    evaluation_id: id.clone(),
                },
            )
        }));
    }
    if let Some(binding) = &sealing.binding {
        let claim = |pin: &medulla_agent_types::agent_component::ComponentDependency,
                     fact: &str| {
            premise(
                &pin.component_id,
                fact,
                PremiseClass::Claim,
                PremiseProvenance::Publisher {
                    component_id: pin.component_id.clone(),
                    definition_digest: pin.definition_digest.clone(),
                },
            )
        };
        out.push(claim(&binding.provider, "retrieval_provider"));
        if let Some(model) = &binding.reasoning_model {
            out.push(claim(model, "reasoning_model"));
        }
    }
    out.extend(knob_premises(sealing));
    out.extend(derived_premises(sealing));
    out.extend(declared_premises(sealing.request));
    out.extend(graph_row_premises(sealing.candidates));
    if let Some(NlBinding {
        source:
            NlChoiceSource::LlmProposal {
                producer,
                prompt_digest,
            },
        template,
        ..
    }) = nl
    {
        out.push(premise(
            &template.component_id,
            "llm_template_proposal",
            PremiseClass::Claim,
            PremiseProvenance::ClaimedMapping {
                text_digest: prompt_digest.clone(),
                producer: producer.clone(),
            },
        ));
    }
    out
}

fn premise_evidence(class: PremiseClass) -> EvidenceClass {
    match class {
        PremiseClass::Definition | PremiseClass::Proof => EvidenceClass::Proof,
        PremiseClass::Observation => EvidenceClass::Observation,
        PremiseClass::Claim => EvidenceClass::Claim,
    }
}

/// The weakest premise, and never stronger than a claim for an answer no
/// calibration licenses (advisory scores and exploration draws).
fn evidence_class(premises: &[PremiseRef], outcome: &StatisticalOutcome) -> EvidenceClass {
    let weakest = premises
        .iter()
        .map(|p| premise_evidence(p.class))
        .max()
        .unwrap_or(EvidenceClass::Proof);
    match outcome {
        StatisticalOutcome::Advisory { .. } | StatisticalOutcome::Explored { .. } => {
            EvidenceClass::Claim
        }
        StatisticalOutcome::Acted { .. } | StatisticalOutcome::Abstained { .. } => weakest,
    }
}

fn resolution(outcome: &StatisticalOutcome) -> ResolutionKind {
    match outcome {
        StatisticalOutcome::Abstained { .. } => ResolutionKind::Abstention,
        StatisticalOutcome::Acted { .. }
        | StatisticalOutcome::Explored { .. }
        | StatisticalOutcome::Advisory { .. } => ResolutionKind::Statistical,
    }
}

/// The receipt an admitted `skill_upgrade` question seals as its answer
/// (MEDULLA-DECISION-ENGINE-R167.3); an abstained one carries none.
fn skill_upgrade_receipt(
    request: &DecideRequest,
    outcome: &StatisticalOutcome,
) -> Option<medulla_decision_types::decision::statistical::outcome::FamilyAnswer> {
    if request.question.kind != QuestionKind::SkillUpgrade
        || matches!(outcome, StatisticalOutcome::Abstained { .. })
    {
        return None;
    }
    let upgrade = super::family::skill_upgrade::upgrade_of(request).ok()?;
    let receipt =
        medulla_decision_types::decision::skill_upgrade_request::SkillUpgradeReceipt::of_request(
            upgrade,
        )
        .ok()?;
    Some(
        medulla_decision_types::decision::statistical::outcome::FamilyAnswer::SkillUpgrade(
            Box::new(receipt),
        ),
    )
}

/// What an admitted `planning_admission` question seals as its answer
/// (MEDULLA-DECISION-ENGINE-R171.3): the view digest compared and no
/// differences. A held plan carries none.
fn planning_admission_answer(
    request: &DecideRequest,
    outcome: &StatisticalOutcome,
) -> Option<medulla_decision_types::decision::statistical::outcome::FamilyAnswer> {
    if request.question.kind != QuestionKind::PlanningAdmission
        || matches!(outcome, StatisticalOutcome::Abstained { .. })
    {
        return None;
    }
    let plan = super::family::planning_admission::plan_of(request)?;
    Some(
        medulla_decision_types::decision::statistical::outcome::FamilyAnswer::PlanningAdmission(
            Box::new(
                medulla_decision_types::decision::planning_admission::PlanningAdmissionAnswer::admitted(plan),
            ),
        ),
    )
}

fn seal(
    sealing: Sealing,
    executed: &Executed,
    nl: Option<NlBinding>,
    task_plan: Option<super::stat_task_plan::Planned>,
    comparative: Option<
        medulla_decision_types::decision::comparative_analysis::ComparativeAnalysisPlan,
    >,
    trajectory_premises: Vec<PremiseRef>,
) -> Result<StatisticalDecisionRecord, String> {
    let inputs = StatisticalInputs {
        harness_evaluation: match executed
            .family
            .as_ref()
            .and_then(|family| family.answer.as_ref())
        {
            Some(FamilyAnswer::HarnessConfig(evidence)) => Some((**evidence).clone()),
            _ => None,
        },
        context_digest: Some(sealing.context_digest.clone()),
        harness_contrast: executed.harness_contrast.clone(),
        feature_schema: sealing.request.feature_schema.clone(),
        head: sealing.request.head.clone(),
        policy: sealing.request.policy.clone(),
        policy_digest: sealing.policy.digest.clone(),
        params: sealing.request.params.clone(),
        classification_rules: sealing.candidates.derived.rules.clone(),
        feature_matrix: feature_matrix(&sealing.candidates.views, sealing.pinned, executed)?,
        shortlist: ShortlistProvenance {
            ann_recall_mode: None,
            now_ms: sealing.ctx.now_ms,
            embedder_model_digest: None,
        },
        exploration: exploration(&sealing, executed),
        belief_slices: bounded(sealing.slices.inputs.clone())?,
        evidence_streams: sealing.request.evidence_streams.clone(),
        graph_rows: bounded(sealing.candidates.graph_rows.clone())?,
    };
    let inputs_digest = digest_text(
        STATISTICAL_INPUTS_DOMAIN,
        &(&sealing.request.question, &inputs),
    );
    let mut premise_list = premises(&sealing, nl.as_ref());
    let task_plan = match task_plan {
        Some(super::stat_task_plan::Planned::Plan(plan, notes)) => {
            premise_list.extend(super::stat_task_plan::premises(&plan, &notes));
            Some(plan)
        }
        Some(super::stat_task_plan::Planned::Abstain { premise, .. }) => {
            premise_list.push(premise);
            None
        }
        None => None,
    };
    if let Some(plan) = &comparative {
        premise_list.extend(super::family::comparative_analysis::premises(plan));
    }
    // A plan that carries a trajectory decision is as strong as its weakest
    // premise (R140.104).
    let trajectory_evidence = task_plan
        .as_ref()
        .is_some_and(|plan| plan.trajectory.is_some());
    premise_list.extend(trajectory_premises);
    if let Some(family) = &executed.family {
        premise_list.extend(family.premises.iter().cloned());
    }
    premise_list.extend(sealing.registry.premises(
        sealing.request,
        executed,
        &sealing.policy.digest,
    ));
    premise_list.extend(medulla_decision_types::decision::evidence_stream::premises(
        sealing.request.evidence_streams.as_slice(),
    ));
    let outcome = executed.ladder.outcome.clone();
    let synthetic = sealing
        .pinned
        .head
        .as_ref()
        .is_some_and(|(head, _)| head.synthetic);
    let mut record = StatisticalDecisionRecord {
        schema_version: STATISTICAL_DECISION_RECORD_SCHEMA_VERSION,
        record_id: format!("decision:{}", inputs_digest.trim_start_matches("sha256:")),
        tenant_id: sealing.ctx.tenant_id.to_string(),
        caller_principal: sealing.principal,
        created_at_ms: sealing.ctx.now_ms,
        question: sealing.request.question.clone(),
        candidate_source: sealing.candidates.record.clone(),
        inputs,
        inputs_digest,
        resolution_kind: if task_plan.is_some() {
            ResolutionKind::Constraint
        } else if executed.family.as_ref().is_some_and(|family| {
            family
                .answer
                .as_ref()
                .and_then(FamilyAnswer::topology_plan)
                .is_some()
        }) {
            ResolutionKind::Optimization
        } else if let Some(route) = &executed.route {
            route.resolution
        } else if let Some(sealed) = executed.family.as_ref().and_then(|a| a.resolution) {
            sealed
        } else {
            sealing
                .registry
                .resolution_kind(&outcome)
                .unwrap_or_else(|| resolution(&outcome))
        },
        evidence_class: if trajectory_evidence {
            premise_list
                .iter()
                .map(|p| premise_evidence(p.class))
                .max()
                .unwrap_or(EvidenceClass::Proof)
        } else if let Some(sealed) = executed.family.as_ref().and_then(|a| a.evidence) {
            sealed
        } else if let Some(hooked) = sealing.registry.evidence_class(&outcome) {
            // A family that names its own evidence class owns it, whatever its
            // premises are.
            hooked
        } else if executed
            .family
            .as_ref()
            .is_some_and(|d| d.premises.iter().any(|p| p.class == PremiseClass::Claim))
        {
            // An answer resting on a connector pack's facts is a claim.
            EvidenceClass::Claim
        } else {
            sealing
                .registry
                .evidence_class(&outcome)
                .unwrap_or_else(|| evidence_class(&premise_list, &outcome))
        },
        trace_fidelity: TraceFidelity::FullStep,
        premises: BoundedVec::new(premise_list)
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?,
        outcome,
        answer: task_plan
            .map(FamilyAnswer::TaskPlan)
            .or_else(|| skill_upgrade_receipt(sealing.request, &executed.ladder.outcome))
            .or_else(|| planning_admission_answer(sealing.request, &executed.ladder.outcome))
            .or_else(|| comparative.map(|plan| FamilyAnswer::ComparativeAnalysis(Box::new(plan))))
            .or_else(|| {
                // A harness configuration is sealed as `inputs.harness_evaluation`,
                // never as the record's `answer` (MEDULLA-DECISION-ENGINE-R128.12).
                executed
                    .family
                    .as_ref()
                    .and_then(|family| family.answer.clone())
                    .filter(|answer| !matches!(answer, FamilyAnswer::HarnessConfig(_)))
            }),
        calibration: executed.ladder.calibration,
        explanation: recorded_explanation(sealing.pinned, executed)?,
        audit: executed.ladder.audit,
        logging_propensities: bounded(executed.ladder.logging.clone())?,
        nl_binding: nl,
        belief: bounded(sealing.slices.points.clone())?,
        synthetic_evidence: synthetic,
        retrieval_binding: sealing.binding.clone(),
        record_digest: String::new(),
    };
    // A load set answer carries the premises it was sealed with
    // (MEDULLA-DECISION-ENGINE-R176.5), not only the ones its family computed.
    if let Some(medulla_decision_types::decision::statistical::outcome::FamilyAnswer::LoadSet(
        answer,
    )) = record.answer.as_mut()
    {
        answer.premises = record.premises.as_slice().to_vec();
    }
    record.record_digest = statistical_record_digest(&record);
    Ok(record)
}
