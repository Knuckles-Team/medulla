//! Evaluate-only explicit DAG planning. EG derives a plan; the host admits it.

use medulla_decision_types::{
    decision::statistical::CandidateSource, decision::statistical::DecideRequest,
    decision::statistical::QuestionKind, decision::statistical::QuestionSafety,
    decision::statistical::StatisticalOutcome, decision::statistical::TypedValue,
};
use medulla_decision_types::{
    decision::task_plan::control_fact, decision::task_plan::plan_explicit_tasks,
    decision::task_plan::task_bindings, decision::task_plan::TaskPlan,
    decision::task_plan::TaskPlanTask,
};
use medulla_decision_types::{
    decision::PremiseClass, decision::PremiseProvenance, decision::PremiseRef,
};
use medulla_types::contract::BoundedVec;

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_agent_types::agent_library::lifecycle::AgentLibraryLifecycle;
use medulla_decision_stats::decision::skill_plan::{plan_over_skills, SkillDocument};
use medulla_decision_types::decision::statistical::TypedParam;
use medulla_decision_types::decision::task_plan::skill_plan::SkillPlanKnobs;
use medulla_decision_types::decision::task_plan::{
    decompose_task, DecomposeKnobs, TaskPlanProposal,
};
use medulla_decision_types::decision::{DecisionPolicy, LibraryCandidateScope};

use super::candidates::library_candidates;
use super::stat_executor::ExecutionContext;
use super::stat_support::{invalid_parameter, resolve_policy};

/// The proposal a `task_plan` question's one typed parameter names: the
/// caller's explicit tasks (`task_plan`), the harness's subtasks and specialist
/// scores (`decomposition`, MEDULLA-DECISION-ENGINE-R134.3) or its atomic
/// subtasks to cover with library skills (`skill_query`, R134.4). Each reaches
/// the one canonicalizer, `plan_explicit_tasks`.
fn proposal_of(
    ctx: &ExecutionContext,
    param: &TypedParam,
    policy: &DecisionPolicy,
) -> Result<TaskPlanProposal, String> {
    match (param.name.as_str(), &param.value) {
        ("task_plan", TypedValue::TaskPlan(proposal)) => Ok((**proposal).clone()),
        ("decomposition", TypedValue::TaskDecomposition(facts)) => {
            decompose_task(facts, &DecomposeKnobs::from_policy(policy)).map_err(invalid_parameter)
        }
        ("skill_query", TypedValue::SkillPlanQuery(query)) => {
            let skills = visible_skills(ctx)?;
            plan_over_skills(query, &skills, &SkillPlanKnobs::from_policy(policy))
                .map_err(invalid_parameter)
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
/// the summary and the declared capabilities, nothing else of the component.
fn visible_skills(ctx: &ExecutionContext) -> Result<Vec<SkillDocument>, String> {
    let scope = LibraryCandidateScope {
        kinds: BoundedVec::new(vec![AgentComponentKind::Skill]).map_err(invalid_parameter)?,
        classification_under: None,
    };
    let read = library_candidates(ctx.store, ctx.tenant_id, &scope)?;
    Ok(read
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == AgentLibraryLifecycle::Published)
        .map(|entry| SkillDocument {
            id: entry.component_id.clone(),
            text: std::iter::once(entry.summary.as_str())
                .chain(entry.declared_capabilities.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect())
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
) -> Result<Option<TaskPlan>, String> {
    if request.question.kind != QuestionKind::TaskPlan
        && !request.params.iter().any(|p| is_plan_param(&p.value))
    {
        return Ok(None);
    }
    let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
    task_plan(ctx, request, &resolved.policy)
}

pub(super) fn task_plan(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Option<TaskPlan>, String> {
    if request.question.kind != QuestionKind::TaskPlan {
        if request.params.iter().any(|p| is_plan_param(&p.value)) {
            return Err(invalid_parameter(
                "task_plan parameter requires task_plan question",
            ));
        }
        return Ok(None);
    }
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "task_plan needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let proposal = proposal_of(ctx, param, policy)?;
    let proposal = &proposal;
    let plan = plan_explicit_tasks(proposal).map_err(invalid_parameter)?;
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
    Ok(Some(plan))
}

pub(super) fn outcome(_plan: &TaskPlan) -> StatisticalOutcome {
    // A canonical plan is not a calibrated score or an authorization grant.
    StatisticalOutcome::Advisory {
        scores: BoundedVec::default(),
        calibrated: false,
    }
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
    out
}

pub(super) fn premises(plan: &TaskPlan) -> Vec<PremiseRef> {
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
        }));
    }
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
    });
    out
}
