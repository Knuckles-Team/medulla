//! Evaluate-only explicit DAG planning. EG derives a plan; the host admits it.

use medulla_decision_policy_types::decision::task_plan::skill_binding::SKILL_BINDING_PARAM;
use medulla_decision_program_types::decision::task_plan::trajectory::TRAJECTORY_PARAM;
use medulla_ingestion_types::skill_plan::skill_binding_stale;
use medulla_types::contract::{BoundedVec, Digest256, ServerErrorCode};
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_program_types::decision::task_plan::control_fact,
    medulla_decision_program_types::decision::task_plan::plan_explicit_tasks,
    medulla_decision_program_types::decision::task_plan::task_bindings,
    medulla_decision_program_types::decision::task_plan::TaskPlan,
    medulla_decision_program_types::decision::task_plan::TaskPlanTask,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::CandidateSource,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::QuestionSafety,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
};

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_library::lifecycle::AgentLibraryLifecycle;
use medulla_decision_stats::decision::skill_plan::{
    plan_over_skills, relevance_reaches, skill_relevances, SkillDocument,
};
use medulla_query_types::solve_scalar::digest_text;
use std::collections::BTreeMap;
use {
    medulla_decision_statistical_types::decision::task_plan::skill_tree::rank_skill_tree,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::OutcomeRef,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::PriorOutcome,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::SkillCandidate,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::SkillHierarchy,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::SkillTreeKnobs,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::TaskSkillTree,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::PARENT_ATTRIBUTE,
    medulla_decision_statistical_types::decision::task_plan::skill_tree::STALE_OUTCOME_EVIDENCE,
};

use super::stat_log::{
    context_digest_of, evaluations_of, visible_entries, visible_entry, LogReader,
};
use medulla_decision_types::decision::code_registry;
use medulla_decision_types::decision::statistical::TypedParam;
use {
    medulla_decision_program_types::decision::task_plan::attach_critical_chain,
    medulla_decision_program_types::decision::task_plan::attach_horizons,
    medulla_decision_program_types::decision::task_plan::decompose_task,
    medulla_decision_program_types::decision::task_plan::CriticalPathKnobs,
    medulla_decision_program_types::decision::task_plan::DecomposeKnobs,
    medulla_decision_program_types::decision::task_plan::HorizonKnobs,
    medulla_decision_program_types::decision::task_plan::TaskPlanProposal,
};
use {
    medulla_decision_program_types::decision::task_plan::NO_SPECIALIST_FOR_SUBTASK,
    medulla_decision_program_types::decision::task_plan::PLAN_PARAMETER_CONFLICT,
};
use {
    medulla_decision_statistical_types::decision::task_plan::skill_plan::SkillPlanKnobs,
    medulla_decision_statistical_types::decision::task_plan::skill_plan::EMPTY_SKILL_INDEX,
    medulla_decision_statistical_types::decision::task_plan::skill_plan::NO_SKILL_FOR_SUBTASK,
};
use {
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::request::LibraryCandidateScope,
};

use super::candidates::library_candidates;
use super::stat_executor::ExecutionContext;
use super::stat_support::{invalid_parameter, resolve_policy};

/// What a `task_plan` question concludes: the canonical plan, or the family's
/// abstention when a proposer cannot cover a subtask (MEDULLA-DECISION-ENGINE-R134.6).
pub(super) enum Planned {
    Plan(Box<TaskPlan>, PlanNotes),
    Abstain {
        code: RefusalCode,
        detail: String,
        premise: PremiseRef,
    },
}

/// What the proposer knew that the canonical plan does not keep: the `Q32`
/// relevance a skill planner chose each task with and whether the engine, not
/// the caller, chose the output aggregation (MEDULLA-DECISION-ENGINE-R134.7).
#[derive(Default)]
pub(super) struct PlanNotes {
    pub(super) relevance_q32: std::collections::BTreeMap<String, i64>,
    pub(super) engine_chose_aggregation: bool,
    /// The premises of a process plan: the workflow it planned, each step binding and the
    /// binding links it dropped (MEDULLA-CQ-ORCH-R704.7); empty for every other plan.
    pub(super) process_premises: Vec<PremiseRef>,
    /// Whether any step of a process plan was bound by the ranker.
    pub(super) process_ranked: bool,
}

/// A proposer's result before canonicalization.
enum Proposal {
    Plan(Box<TaskPlanProposal>, PlanNotes),
    Abstain(Planned),
}

/// The abstention a proposer's coded failure is: only the coverage codes
/// abstain; every other planner failure is a refused request.
fn abstention(message: &str, param: &str) -> Option<Planned> {
    let (code, detail) = message.split_once(": ").unwrap_or((message, ""));
    if ![
        NO_SKILL_FOR_SUBTASK,
        NO_SPECIALIST_FOR_SUBTASK,
        EMPTY_SKILL_INDEX,
    ]
    .contains(&code)
    {
        return None;
    }
    let code = code_registry::resolve(code)?;
    let subject = if code.as_str() == EMPTY_SKILL_INDEX {
        "skill_index".to_string()
    } else {
        detail.to_string()
    };
    Some(Planned::Abstain {
        code,
        detail: detail.to_string(),
        premise: PremiseRef {
            subject,
            fact: format!(
                "task_plan_abstention:code={};subtask={detail}",
                code.as_str()
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: format!("params.{param}"),
            },
            trust_tier: None,
        },
    })
}

fn proposed(result: Result<TaskPlanProposal, String>, param: &str) -> Result<Proposal, String> {
    match result {
        Ok(proposal) => Ok(Proposal::Plan(Box::new(proposal), PlanNotes::default())),
        Err(message) => match abstention(&message, param) {
            Some(planned) => Ok(Proposal::Abstain(planned)),
            None => Err(invalid_parameter(message)),
        },
    }
}

/// The proposal a `task_plan` question's one typed parameter names: the
/// caller's explicit tasks (`task_plan`), the harness's subtasks and specialist
/// scores (`decomposition`, MEDULLA-DECISION-ENGINE-R134.3) or its atomic
/// subtasks to cover with library skills (`skill_query`, R134.4). Each reaches
/// the one canonicalizer, `plan_explicit_tasks`.
fn proposal_of(
    ctx: &ExecutionContext,
    param: &TypedParam,
    policy: &DecisionPolicy,
) -> Result<Proposal, String> {
    match (param.name.as_str(), &param.value) {
        ("task_plan", TypedValue::TaskPlan(proposal)) => {
            Ok(Proposal::Plan(proposal.clone(), PlanNotes::default()))
        }
        ("decomposition", TypedValue::TaskDecomposition(facts)) => proposed(
            decompose_task(facts, &DecomposeKnobs::from_policy(policy)),
            "decomposition",
        )
        .map(|proposal| match proposal {
            Proposal::Plan(plan, _) => Proposal::Plan(
                plan,
                PlanNotes {
                    relevance_q32: Default::default(),
                    engine_chose_aggregation: true,
                    process_premises: Vec::new(),
                    process_ranked: false,
                },
            ),
            other => other,
        }),
        ("skill_query", TypedValue::SkillPlanQuery(query)) => {
            let (skills, _) = visible_skills(ctx)?;
            let planned = plan_over_skills(query, &skills, &SkillPlanKnobs::from_policy(policy));
            let relevance = planned
                .as_ref()
                .map(|p| p.relevance_q32.clone())
                .unwrap_or_default();
            proposed(planned.map(|p| p.proposal), "skill_query").map(|proposal| match proposal {
                Proposal::Plan(plan, _) => Proposal::Plan(
                    plan,
                    PlanNotes {
                        relevance_q32: relevance,
                        engine_chose_aggregation: false,
                        process_premises: Vec::new(),
                        process_ranked: false,
                    },
                ),
                other => other,
            })
        }
        (_, TypedValue::TaskPlan(_)) => Err(invalid_parameter(
            "task_plan parameter must be named task_plan",
        )),
        (_, TypedValue::TaskDecomposition(_)) => Err(invalid_parameter(
            "decomposition parameter must be named decomposition",
        )),
        (_, TypedValue::SkillPlanQuery(_)) => Err(invalid_parameter(
            "skill_query parameter must be named skill_query",
        )),
        _ => Err(invalid_parameter(
            "task_plan requires a typed task_plan proposal, decomposition or skill_query",
        )),
    }
}

/// The published skills the tenant sees, as the text relevance scores against:
/// the summary and the declared capabilities, nothing else of the component;
/// and the `parent` each declares.
fn visible_skills(ctx: &ExecutionContext) -> Result<(Vec<SkillDocument>, SkillHierarchy), String> {
    let scope = LibraryCandidateScope {
        kinds: BoundedVec::new(vec![AgentComponentKind::Skill]).map_err(invalid_parameter)?,
        classification_under: None,
    };
    let read = library_candidates(ctx.store, ctx.tenant_id, &scope)?;
    let mut hierarchy = SkillHierarchy::default();
    let mut documents = Vec::new();
    for entry in read
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == AgentLibraryLifecycle::Published)
    {
        hierarchy.visible.insert(entry.component_id.clone());
        if let Some(parent) = entry.attributes.get(PARENT_ATTRIBUTE) {
            hierarchy
                .parents
                .insert(entry.component_id.clone(), parent.clone());
        }
        documents.push(SkillDocument {
            id: entry.component_id.clone(),
            text: std::iter::once(entry.summary.as_str())
                .chain(entry.declared_capabilities.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        });
    }
    Ok((documents, hierarchy))
}

fn stale(detail: impl std::fmt::Display) -> String {
    invalid_parameter(format!("{STALE_OUTCOME_EVIDENCE}: {detail}"))
}

/// What one logged record's stored evaluations say, per evaluated skill.
struct RecordOutcomes {
    reference: OutcomeRef,
    /// `selected_agent -> (successes, trials)`; censored evaluations count nowhere.
    by_skill: BTreeMap<String, (u64, u64)>,
}

/// The stored outcome of `record_id`: its revision is the evaluation count
/// (the log only appends) and its digest covers every stored evaluation.
fn record_outcomes(
    ctx: &ExecutionContext,
    record_id: &str,
) -> Result<Option<RecordOutcomes>, String> {
    let stored = evaluations_of(ctx.store, ctx.tenant_id, record_id)?;
    if stored.is_empty() {
        return Ok(None);
    }
    let mut facts: Vec<(&str, Option<bool>, &str)> = stored
        .iter()
        .map(|s| {
            (
                s.evaluation.evaluation_id.as_str(),
                s.evaluation.success,
                s.evaluation.selected_agent.as_str(),
            )
        })
        .collect();
    facts.sort();
    let mut by_skill: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (_, success, agent) in &facts {
        if let Some(success) = success {
            let slot = by_skill.entry((*agent).to_string()).or_default();
            slot.0 += u64::from(*success);
            slot.1 += 1;
        }
    }
    Ok(Some(RecordOutcomes {
        reference: OutcomeRef {
            record_id: record_id.to_string(),
            outcome_revision: stored.len() as u64,
            outcome_digest: digest_text("eg/skill-tree-outcome/v1", &(record_id, &facts)),
        },
        by_skill,
    }))
}

/// The outcomes a tree leans on. With pins, exactly the pinned records, each
/// of which must equal its stored outcome and share the context; without,
/// the caller's own matching records (same tenant, principal and context),
/// newest first up to the reference bound.
fn outcome_evidence(
    ctx: &ExecutionContext,
    reader: &LogReader,
    context: &str,
    pins: &[OutcomeRef],
) -> Result<Vec<RecordOutcomes>, String> {
    use medulla_decision_statistical_types::decision::task_plan::skill_tree::MAX_OUTCOME_REFS;
    let shares_context =
        |record: &medulla_decision_types::decision::statistical::StatisticalDecisionRecord| {
            record.caller_principal == reader.principal
                && record.inputs.context_digest.as_deref() == Some(context)
        };
    let mut out = Vec::new();
    if pins.is_empty() {
        let mut matching: Vec<_> = visible_entries(ctx.store, reader)?
            .into_iter()
            .filter(|entry| shares_context(&entry.record))
            .map(|entry| (entry.record.created_at_ms, entry.record.record_id.clone()))
            .collect();
        matching.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, record_id) in matching {
            if out.len() == MAX_OUTCOME_REFS {
                break;
            }
            out.extend(record_outcomes(ctx, &record_id)?);
        }
        return Ok(out);
    }
    for pin in pins {
        let entry = visible_entry(ctx.store, reader, &pin.record_id)?
            .ok_or_else(|| stale(format!("{} is not a visible record", pin.record_id)))?;
        if !shares_context(&entry.record) {
            return Err(stale(format!(
                "{} does not share the request context",
                pin.record_id
            )));
        }
        let current = record_outcomes(ctx, &pin.record_id)?
            .filter(|current| current.reference == *pin)
            .ok_or_else(|| stale(format!("{} differs from the stored outcome", pin.record_id)))?;
        out.push(current);
    }
    Ok(out)
}

/// Attach the ranked skill tree of each subtask of a `skill_query` plan and
/// re-bind the plan digest over them (MEDULLA-DECISION-ENGINE-R134.9).
fn with_skill_trees(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    principal: &str,
    policy: &DecisionPolicy,
    mut plan: TaskPlan,
) -> Result<TaskPlan, String> {
    let Some(query) = request.params.iter().find_map(|p| match &p.value {
        TypedValue::SkillPlanQuery(query) => Some(query),
        _ => None,
    }) else {
        return Ok(plan);
    };
    let (skills, hierarchy) = visible_skills(ctx)?;
    let plan_knobs = SkillPlanKnobs::from_policy(policy);
    let rows = skill_relevances(query, &skills, &plan_knobs).map_err(invalid_parameter)?;
    let context = context_digest_of(&request.question.question_id, request.params.as_slice());
    let reader = LogReader {
        tenant_id: ctx.tenant_id.to_string(),
        principal: principal.to_string(),
        roles: Vec::new(),
        retention: super::stat_retention::Retention::none(),
    };
    let evidence = outcome_evidence(ctx, &reader, &context, query.outcome_pins.as_slice())?;
    let knobs = SkillTreeKnobs::from_policy(policy);
    let mut trees: Vec<TaskSkillTree> = Vec::with_capacity(rows.len());
    for (task_id, row) in rows {
        let candidates = row
            .into_iter()
            .filter(|(_, relevance)| {
                relevance_reaches(*relevance, plan_knobs.relevance_floor_micros)
            })
            .map(|(skill_id, relevance_q32)| {
                let outcomes = evidence
                    .iter()
                    .filter_map(|record| {
                        let (successes, trials) = *record.by_skill.get(&skill_id)?;
                        Some(PriorOutcome {
                            reference: record.reference.clone(),
                            successes,
                            trials,
                        })
                    })
                    .collect();
                SkillCandidate {
                    skill_id,
                    relevance_q32,
                    outcomes,
                }
            })
            .collect();
        trees.push(
            rank_skill_tree(&task_id, &context, candidates, &hierarchy, &knobs)
                .map_err(invalid_parameter)?,
        );
    }
    plan.skill_trees = BoundedVec::new(trees).map_err(invalid_parameter)?;
    plan.plan_digest = digest_text(
        "eg/task-plan-skill-trees/v1",
        &(&plan.plan_digest, &plan.skill_trees),
    );
    Ok(plan)
}

fn is_plan_param(value: &TypedValue) -> bool {
    matches!(
        value,
        TypedValue::TaskPlan(_) | TypedValue::TaskDecomposition(_) | TypedValue::SkillPlanQuery(_)
    )
}

/// The plan of a request that carries one, resolving the decision policy its
/// knobs are read from. Other requests have no plan and read nothing.
pub(super) fn plan_of_request(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    principal: &str,
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
) -> Result<Option<Planned>, String> {
    if request.question.kind == QuestionKind::ProcessPlan {
        let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
        return super::family::process_plan::plan(ctx, request, &resolved.policy, graph_view)
            .map(Some);
    }
    if request.question.kind != QuestionKind::TaskPlan
        && !request.params.iter().any(|p| is_plan_param(&p.value))
    {
        return Ok(None);
    }
    let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
    match task_plan(ctx, request, &resolved.policy)? {
        Some(Planned::Plan(plan, notes)) => {
            check_skill_binding(ctx, request, graph_view, &plan)?;
            let plan = with_skill_trees(ctx, request, principal, &resolved.policy, *plan)?;
            Ok(Some(Planned::Plan(Box::new(plan), notes)))
        }
        other => Ok(other),
    }
}

fn skill_binding_stale_error(detail: &str) -> String {
    format!("{}: {detail}", ServerErrorCode::SkillBindingStale.as_str())
}

/// Whether a node belongs to the asking tenant; a node that carries no tenant
/// belongs to the graph and counts.
fn visible_to(row: &serde_json::Map<String, serde_json::Value>, tenant: &str) -> bool {
    row.get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|owner| owner == tenant)
}

/// The pinned skill binding of a request, when it carries one.
pub(super) fn skill_binding_of(
    request: &DecideRequest,
) -> Option<&medulla_decision_policy_types::decision::task_plan::skill_binding::SkillBindingPin> {
    request.params.iter().find_map(|param| match &param.value {
        TypedValue::SkillBinding(pin) if param.name == SKILL_BINDING_PARAM => Some(pin.as_ref()),
        _ => None,
    })
}

/// Refuse a plan whose pinned skill binding no longer matches the stored
/// compilation (MEDULLA-REPO-INGEST-R002.1.3). The refusal returns before any
/// decision record is written; a request without the param is unchanged.
fn check_skill_binding(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
    plan: &TaskPlan,
) -> Result<(), String> {
    let Some(pin) = skill_binding_of(request) else {
        return Ok(());
    };
    let not_found = || skill_binding_stale_error("binding not found");
    let row = graph_view
        .and_then(|view| view.node_row_object(&pin.node_id))
        .filter(|row| {
            visible_to(row, ctx.tenant_id)
                && row.get("type").and_then(serde_json::Value::as_str) == Some("SkillCompilation")
        })
        .ok_or_else(not_found)?;
    let stored = serde_json::Value::Object(row);
    let document = Digest256::parse(&pin.document_digest).map_err(invalid_parameter)?;
    let capability = Digest256::parse(medulla_capabilities::catalog::contract_catalog_digest())
        .map_err(invalid_parameter)?;
    // Without a graph schema digest only the type comparison is skipped: the
    // stored value stands in for the current one.
    let types = ctx
        .type_digest
        .or_else(|| {
            stored
                .get("type_digest")
                .and_then(serde_json::Value::as_str)
                .and_then(|hex| Digest256::parse(hex).ok())
        })
        .unwrap_or_else(|| Digest256::from_bytes([0; 32]));
    if let Some(name) = skill_binding_stale(&stored, &document, &capability, &types) {
        return Err(skill_binding_stale_error(&format!("{name} differs")));
    }
    let plan_digest = Digest256::sha256(plan.plan_digest.as_bytes());
    if stored
        .get("plan_digest")
        .and_then(serde_json::Value::as_str)
        != Some(plan_digest.to_hex().as_str())
    {
        return Err(skill_binding_stale_error("plan_digest differs"));
    }
    Ok(())
}

/// The params a plan-producing question carries beyond the plan itself when it
/// asks for admission: the `planning_admission` plan and its `graph`.
fn has_admission_inputs(request: &DecideRequest) -> bool {
    super::family::planning_admission::plan_of(request).is_some()
}

pub(super) fn task_plan(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Option<Planned>, String> {
    if request.question.kind != QuestionKind::TaskPlan {
        if request.params.iter().any(|p| {
            is_plan_param(&p.value)
                || matches!(
                    p.value,
                    TypedValue::Trajectory(_) | TypedValue::SkillBinding(_)
                )
        }) {
            return Err(invalid_parameter(
                "task_plan parameter requires task_plan question",
            ));
        }
        return Ok(None);
    }
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(
            "task_plan needs ordinary safety, no head or belief slices",
        ));
    }
    let proposers = request
        .params
        .iter()
        .filter(|p| is_plan_param(&p.value))
        .count();
    if proposers != 1 {
        return Err(invalid_parameter(format!(
            "{PLAN_PARAMETER_CONFLICT}: a task_plan question carries exactly one of task_plan, \
             decomposition or skill_query, got {proposers}"
        )));
    }
    // At most one trajectory query rides on the plan (MEDULLA-DECISION-ENGINE-R140.104).
    let trajectories: Vec<_> = request
        .params
        .iter()
        .filter(|p| matches!(p.value, TypedValue::Trajectory(_)))
        .collect();
    if trajectories.len() > 1 || trajectories.iter().any(|p| p.name != TRAJECTORY_PARAM) {
        return Err(invalid_parameter(
            "a task_plan question carries at most one trajectory query, named trajectory",
        ));
    }
    // At most one skill binding rides on the plan, named skill_binding; it reads
    // its node from the graph named by the same `graph` param a plan admission
    // uses (MEDULLA-REPO-INGEST-R002.1.3).
    let bindings: Vec<_> = request
        .params
        .iter()
        .filter(|p| matches!(p.value, TypedValue::SkillBinding(_)))
        .collect();
    if bindings.len() > 1 || bindings.iter().any(|p| p.name != SKILL_BINDING_PARAM) {
        return Err(invalid_parameter(
            "a task_plan question carries at most one skill binding, named skill_binding",
        ));
    }
    let admission = usize::from(has_admission_inputs(request));
    let graph_param = usize::from(admission == 1 || !bindings.is_empty());
    if request.params.len() != 1 + admission + graph_param + trajectories.len() + bindings.len() {
        return Err(invalid_parameter(
            "task_plan takes its one proposer parameter and no other, besides a plan to admit, the graph it reads, one trajectory query and one skill binding",
        ));
    }
    let param = request
        .params
        .iter()
        .find(|p| is_plan_param(&p.value))
        .ok_or_else(|| invalid_parameter("task_plan carries no proposer parameter"))?;
    let proposal = match proposal_of(ctx, param, policy)? {
        Proposal::Plan(proposal, notes) => (proposal, notes),
        Proposal::Abstain(planned) => return Ok(Some(planned)),
    };
    let (proposal, notes) = proposal;
    let proposal = &proposal;
    let mut plan = plan_explicit_tasks(proposal).map_err(invalid_parameter)?;
    attach_critical_chain(&mut plan, &CriticalPathKnobs::from_policy(policy))
        .map_err(invalid_parameter)?;
    attach_horizons(&mut plan, &HorizonKnobs::from_policy(policy)).map_err(invalid_parameter)?;
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter("task_plan requires declared task ids"));
    };
    let mut task_ids: Vec<&str> = plan
        .tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .collect();
    task_ids.sort();
    if options
        .iter()
        .map(|o| o.option_id.as_str())
        .collect::<Vec<_>>()
        != task_ids
        || options
            .iter()
            .any(|o| !o.classification.is_empty() || !o.numbers.is_empty() || !o.texts.is_empty())
    {
        return Err(invalid_parameter(
            "declared candidates must be exactly the sorted task ids without facts",
        ));
    }
    Ok(Some(Planned::Plan(Box::new(plan), notes)))
}

pub(super) fn outcome(_plan: &TaskPlan) -> StatisticalOutcome {
    // A canonical plan is not a calibrated score or an authorization grant.
    StatisticalOutcome::advisory(BoundedVec::default(), false)
}

/// A tool step's call and each binding are caller claims; neither grants
/// authority to invoke the tool (MEDULLA-DECISION-ENGINE-R134.111.11).
fn call_premises(task: &TaskPlanTask) -> Vec<PremiseRef> {
    let Some(call) = &task.call else {
        return Vec::new();
    };
    let bindings = task_bindings(task);
    let claim = |fact: String, field: &str| PremiseRef {
        subject: task.task_id.clone(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: field.to_string(),
        },
        trust_tier: None,
    };
    let references: Vec<&str> = bindings.iter().map(|b| b.reference.as_str()).collect();
    let mut out = vec![claim(
        format!(
            "task_plan_call:server={};tool={};bindings={}",
            call.server,
            call.tool,
            references.join(",")
        ),
        "params.task_plan.tasks.call",
    )];
    out.extend(bindings.iter().map(|binding| {
        claim(
            format!(
                "task_plan_binding:pointer={};source_task={};reference={}",
                binding.pointer, binding.source_task, binding.reference
            ),
            "params.task_plan.tasks.call.arguments",
        )
    }));
    // One premise per foreach step names the source reference and the bound
    // (MEDULLA-DECISION-ENGINE-R134.111.21).
    if let Some(foreach) = &task.foreach {
        out.push(claim(
            format!(
                "task_plan_foreach:source={};max_items={}",
                foreach.source.reference, foreach.max_items
            ),
            "params.task_plan.tasks.foreach",
        ));
    }
    out
}

pub(super) fn premises(plan: &TaskPlan, notes: &PlanNotes) -> Vec<PremiseRef> {
    let mut out: Vec<PremiseRef> = plan
        .tasks
        .iter()
        .map(|task| PremiseRef {
            subject: task.task_id.clone(),
            fact: format!(
                "task_plan_claim:dependencies={};prompt_digest={}",
                task.depends_on.as_slice().join(","),
                medulla_query_types::solve_scalar::digest_text(
                    "eg/task-plan-prompt/v1",
                    &task.prompt
                )
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: "params.task_plan.tasks".to_string(),
            },
            trust_tier: None,
        })
        .collect();
    for task in plan.tasks.iter() {
        out.extend(call_premises(task));
        // A control step is a caller claim too (MEDULLA-DECISION-ENGINE-R134.111.17).
        out.extend(control_fact(task).map(|fact| PremiseRef {
            subject: task.task_id.clone(),
            fact,
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: "params.task_plan.tasks.control".to_string(),
            },
            trust_tier: None,
        }));
    }
    for tree in plan.skill_trees.iter() {
        out.extend(tree.nodes.iter().map(|node| PremiseRef {
            subject: tree.task_id.clone(),
            fact: format!(
                "skill_node:task={};skill={};parent={};rank={};score_q32={}",
                tree.task_id,
                node.skill_id,
                node.parent_skill_id.as_deref().unwrap_or(""),
                node.rank,
                node.score_q32
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: "params.skill_query".to_string(),
            },
            trust_tier: None,
        }));
    }
    out.extend(assignment_premises(plan, notes));
    out.extend(notes.process_premises.iter().cloned());
    out.extend(critical_chain_premises(plan));
    out.extend(horizon_premises(plan));
    out.push(PremiseRef {
        subject: plan.policy_revision.clone(),
        fact: format!(
            "bounded_explicit_dag:max_steps={};max_concurrency={};plan_digest={}",
            plan.max_steps, plan.max_concurrency, plan.plan_digest
        ),
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Policy {
            policy_digest: plan.policy_digest.clone(),
        },
        trust_tier: None,
    });
    out
}

/// The planner premises (MEDULLA-DECISION-ENGINE-R134.7): one `Claim` per task
/// that names its assignee, relevance (skill-planned tasks only) and whether it
/// has no dependency, and one `Definition` for the output task's aggregation,
/// saying whether the engine chose the operator.
fn assignment_premises(plan: &TaskPlan, notes: &PlanNotes) -> Vec<PremiseRef> {
    let independent =
        medulla_decision_program_types::decision::task_plan::independent_ids(plan.tasks.as_slice());
    let mut out: Vec<PremiseRef> = plan
        .tasks
        .iter()
        .filter_map(|task| {
            let assignee = task.assignee.as_deref()?;
            let relevance = notes
                .relevance_q32
                .get(&task.task_id)
                .map(|q| format!("relevance_q32={q};"))
                .unwrap_or_default();
            Some(PremiseRef {
                subject: task.task_id.clone(),
                fact: format!(
                    "task_plan_assignment:assignee={assignee};{relevance}independent={}",
                    independent.contains(task.task_id.as_str())
                ),
                class: PremiseClass::Claim,
                provenance: PremiseProvenance::Request {
                    field: "params.task_plan.tasks.assignee".to_string(),
                },
                trust_tier: None,
            })
        })
        .collect();
    let output = plan
        .tasks
        .iter()
        .find(|task| task.task_id == plan.output_task_id);
    if let Some(aggregation) = output.and_then(|task| task.aggregation.as_ref()) {
        out.push(PremiseRef {
            subject: plan.output_task_id.clone(),
            fact: format!(
                "task_plan_aggregation:op={};default={}",
                aggregation.op.name(),
                notes.engine_chose_aggregation
            ),
            class: PremiseClass::Definition,
            provenance: PremiseProvenance::Policy {
                policy_digest: plan.policy_digest.clone(),
            },
            trust_tier: None,
        });
    }
    out
}

/// The critical-chain premises (MEDULLA-DECISION-ENGINE-R134.15): one `Claim`
/// per task naming the duration and weight the caller supplied, and one
/// `Definition` for the chain the engine derived from them.
fn critical_chain_premises(plan: &TaskPlan) -> Vec<PremiseRef> {
    let Some(chain) = &plan.critical_chain else {
        return Vec::new();
    };
    let mut out: Vec<PremiseRef> = plan
        .tasks
        .iter()
        .filter_map(|task| {
            Some(PremiseRef {
                subject: task.task_id.clone(),
                fact: format!(
                    "critical_path_input:task={};duration_ms={};weight={}",
                    task.task_id, task.duration_ms?, task.priority_weight?
                ),
                class: PremiseClass::Claim,
                provenance: PremiseProvenance::Request {
                    field: "params.task_plan.tasks.duration_ms".to_string(),
                },
                trust_tier: None,
            })
        })
        .collect();
    out.push(PremiseRef {
        subject: plan.output_task_id.clone(),
        fact: format!(
            "task_plan_critical_chain:steps={};total_weighted_duration_ms={};weight_scale={};\
             input_digest={}",
            chain.critical_chain_step_ids.join(","),
            chain.total_weighted_duration_ms,
            chain.weight_scale,
            chain.input_digest
        ),
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Policy {
            policy_digest: plan.policy_digest.clone(),
        },
        trust_tier: None,
    });
    out
}

/// One `Claim` per horizon naming its steps (MEDULLA-DECISION-ENGINE-R134.11).
fn horizon_premises(plan: &TaskPlan) -> Vec<PremiseRef> {
    let Some(partition) = &plan.horizons else {
        return Vec::new();
    };
    partition
        .horizons
        .iter()
        .map(|horizon| PremiseRef {
            subject: plan.output_task_id.clone(),
            fact: format!(
                "horizon:index={};steps={}",
                horizon.index,
                horizon.step_ids.join(",")
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Policy {
                policy_digest: plan.policy_digest.clone(),
            },
            trust_tier: None,
        })
        .collect()
}
