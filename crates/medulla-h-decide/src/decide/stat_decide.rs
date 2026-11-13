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
use medulla_decision_types::{
    decision::statistical::nl::NlBinding, decision::statistical::nl::NlChoiceSource,
};
use medulla_decision_types::{
    decision::statistical::CandidateSource, decision::statistical::DecideRequest,
    decision::statistical::DecisionBatch, decision::statistical::ExplorationRecord,
    decision::statistical::FeatureMatrixRef, decision::statistical::QuestionKind,
    decision::statistical::ShortlistProvenance, decision::statistical::StatisticalDecisionRecord,
    decision::statistical::StatisticalErrorCode, decision::statistical::StatisticalInputs,
    decision::statistical::StatisticalOutcome,
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
use super::stat_log::{fill_outcome_rates, LogReader};
use super::stat_nl::binding;
use super::stat_support::{
    refusal, require_supported_question_kind, resolve_policy, ResolvedPolicy,
};
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
        require_supported_question_kind(request.question.kind)?;
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
                .map(Some)
            } else if request.question.kind == QuestionKind::EditConcentration {
                super::stat_concentration::concentration_view(state, verified.agent_id(), &request)
                    .await
                    .map(Some)
            } else if request.question.kind == QuestionKind::HeldItemAdmission {
                super::family::held_item_view(state, verified.agent_id(), &request)
                    .await
                    .map(Some)
            } else if request.question.kind == QuestionKind::CommandConfig {
                super::family::command_view(state, verified.agent_id(), &request)
                    .await
                    .map(Some)
            } else if request.question.kind == QuestionKind::FanoutAggregate {
                super::family::fanout_view(state, verified.agent_id(), verified.tenant(), &request)
                    .await
                    .map(Some)
            } else if request.question.kind == QuestionKind::RuntimeQuota {
                super::family::quota_view(state, verified.agent_id(), &request).await
            } else if request.question.kind == QuestionKind::InstructionDisclosure {
                super::family::disclosure_view(state, verified.agent_id(), &request)
                    .await
                    .map(Some)
            } else if request.question.kind == QuestionKind::ContextPackage {
                // A `context_package` question declares its candidates and checks
                // each against its cited source node in the graph named by its
                // `source_graph` param, under the same ACL and RLS filter.
                let graph = super::family::context_package::source_graph(&request)?;
                super::stat_support::filtered_view(state, verified.agent_id(), graph)
                    .await
                    .map(Some)
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
                let ctx = ExecutionContext {
                    store: store.as_ref(),
                    tenant_id: &request.tenant_id,
                    now_ms,
                    server_secret: secret.as_bytes(),
                    matched: &[],
                };
                decide_blocking(&ctx, &request, graph_view.as_ref(), principal, started)
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
            &principal,
            registry,
        )
    })?;
    let (pinned, policy) = telemetry::stage(Stage::Constraints, family, tenant, || {
        registry.constraints(request)?;
        let pinned = pinned_inputs(ctx, &request.feature_schema, request.head.as_ref())?;
        let policy = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
        Ok::<_, String>((pinned, policy))
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
        ..*ctx
    };
    fill_outcome_rates(
        ctx.store,
        &reader,
        &pinned.schema,
        policy.statistical.tenant_public_features,
        &mut candidates.views,
    )?;
    // The kind's family concludes its specialized answer; the shared
    // matrix ladder is the fall-through (R001.101).
    let executed = telemetry::stage(Stage::Solve, family, tenant, || {
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
                None => execute(ctx, request, &pinned, &policy, &candidates.views)?,
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
            },
            &executed,
            nl,
            task_plan,
            comparative,
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
        QuestionKind::TaskPlan | QuestionKind::ThoughtSearch
    ) {
        return None; // Explicit planning and thought search never draw.
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
        QuestionKind::TaskPlan
            if sealing.request.params.iter().any(|p| {
                matches!(
                    p.value,
                    medulla_decision_types::decision::statistical::TypedValue::SkillPlanQuery(_)
                )
            }) =>
        {
            medulla_decision_types::decision::knob_families::task_plan::SKILL_TREE_KNOBS
        }
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
        QuestionKind::ProbeVerdict => {
            medulla_decision_types::decision::knob_families::red_team::KNOBS
        }
        QuestionKind::CriterionReview => {
            medulla_decision_types::decision::knob_families::criterion_review::KNOBS
        }
        QuestionKind::DelegationAdmission => medulla_decision_types::decision::delegation::KNOBS,
        QuestionKind::RunAdjustment => medulla_decision_types::decision::run_adjustment::KNOBS,
        QuestionKind::PathPreference => medulla_decision_types::decision::path_success::KNOBS,
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
    out.extend(knob_premises(sealing));
    out.extend(derived_premises(sealing));
    out.extend(declared_premises(sealing.request));
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

fn seal(
    sealing: Sealing,
    executed: &Executed,
    nl: Option<NlBinding>,
    task_plan: Option<super::stat_task_plan::Planned>,
    comparative: Option<
        medulla_decision_types::decision::comparative_analysis::ComparativeAnalysisPlan,
    >,
) -> Result<StatisticalDecisionRecord, String> {
    let inputs = StatisticalInputs {
        harness_evaluation: None,
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
    if let Some(topology) = &executed.topology {
        premise_list.extend(topology.premises.iter().cloned());
    }
    if let Some(thought) = &executed.thought {
        premise_list.extend(thought.premises.iter().cloned());
    }
    premise_list.extend(sealing.registry.premises(
        sealing.request,
        executed,
        &sealing.policy.digest,
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
        } else if executed.topology.is_some() {
            ResolutionKind::Optimization
        } else if let Some(route) = &executed.route {
            route.resolution
        } else {
            sealing
                .registry
                .resolution_kind(&outcome)
                .unwrap_or_else(|| resolution(&outcome))
        },
        evidence_class: sealing
            .registry
            .evidence_class(&outcome)
            .unwrap_or_else(|| evidence_class(&premise_list, &outcome)),
        trace_fidelity: TraceFidelity::FullStep,
        premises: BoundedVec::new(premise_list)
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?,
        outcome,
        answer: task_plan
            .map(|plan| {
                medulla_decision_types::decision::statistical::outcome::FamilyAnswer::TaskPlan(
                    Box::new(plan),
                )
            })
            .or_else(|| {
                comparative.map(|plan| {
                    medulla_decision_types::decision::statistical::outcome::FamilyAnswer::ComparativeAnalysis(
                        Box::new(plan),
                    )
                })
            })
            .or_else(|| {
                executed.thought.as_ref().map(|thought| {
                    medulla_decision_types::decision::statistical::outcome::FamilyAnswer::ThoughtSearch(
                        Box::new(thought.answer.clone()),
                    )
                })
            })
            .or_else(|| {
                executed.topology.as_ref().map(|topology| {
                    medulla_decision_types::decision::statistical::outcome::FamilyAnswer::Topology(
                        topology.plan.clone(),
                    )
                })
            }),
        calibration: executed.ladder.calibration,
        explanation: recorded_explanation(sealing.pinned, executed)?,
        audit: executed.ladder.audit,
        logging_propensities: bounded(executed.ladder.logging.clone())?,
        nl_binding: nl,
        belief: bounded(sealing.slices.points.clone())?,
        synthetic_evidence: synthetic,
        record_digest: String::new(),
    };
    record.record_digest = statistical_record_digest(&record);
    Ok(record)
}
