//! Bounded, explicit dependency planning (MEDULLA-DECISION-ENGINE-R134.111).
//!
//! Prompts and edges are caller claims, never authorization or inferred policy.
//! The engine canonicalizes a DAG; Arbora admits it and AU executes it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;

use medulla_query_types::solve_scalar::digest_text;

mod aggregation;
pub mod branch_breadth;
mod call;
mod control;
pub mod critical_path;
mod decompose;
pub mod horizon;
pub mod pairwise;
pub mod skill_plan;
pub mod skill_tree;
pub mod trajectory_budget;
pub mod trajectory_projection;
pub mod trajectory_refusal;
pub use aggregation::{
    AggregationOp, TaskPlanAggregation, PLAN_AGGREGATION_INVALID, PLAN_AGGREGATION_NOT_OUTPUT,
};
pub use call::{
    task_bindings, TaskPlanBinding, TaskPlanCall, BINDING_KEY, MAX_BINDINGS_PER_CALL,
    MAX_BINDING_PATH_BYTES, MAX_BINDING_PATH_SEGMENTS, MAX_CALL_ARGUMENT_BYTES, MAX_TOOL_STEPS,
};
pub use control::{
    control_fact, ApprovalSpec, BranchArm, BranchComparison, BranchCondition,
    BranchDecisionCondition, BranchKind, BranchSpec, CompareOp, DecisionSpec, JoinSpec,
    APPROVAL_TIMEOUT_OUT_OF_RANGE, BRANCH_ARM_UNKNOWN, BRANCH_JOIN_UNMATCHED, BRANCH_NO_DEFAULT,
    CONTROL_STEP_INVALID, MAX_APPROVAL_TIMEOUT_S, MIN_APPROVAL_TIMEOUT_S, STEP_KIND_CONFLICT,
};
pub use critical_path::{
    attach_critical_chain, critical_chain, CriticalChain, CriticalPathKnobs,
    CRITICAL_PATH_INVALID_DURATION, CRITICAL_PATH_OVERFLOW, CRITICAL_PATH_WEIGHT_MISSING,
};
pub use decompose::{
    decompose_task, independent_count, independent_ids, topology_requirements, DecomposeFacts,
    DecomposeKnobs, DecomposeSubtask, SpecialistScore, NO_SPECIALIST_FOR_SUBTASK,
};
pub use horizon::{
    attach_horizons, check_horizon_checkpoint, checkpoint_key, partition_horizons, Horizon,
    HorizonCheckpoint, HorizonKnobs, HorizonPartition,
};

pub const TASK_PLAN_POLICY_REVISION: &str = "bounded-explicit-dag-v2";

/// The Agent Library component id a host publishes the canonical task-plan
/// feature schema under (MEDULLA-DECISION-ENGINE-R134.111.7).
pub const TASK_PLAN_FEATURE_SCHEMA_COMPONENT_ID: &str = "task-plan-features";

/// The canonical feature schema a `task_plan` Decide request pins.
///
/// A task-plan decision resolves by constraint and never builds a feature
/// matrix, but every Decide request pins a published `FeatureSchema`. One
/// engine-defined body means every host publishes the same bytes, so one
/// tenant's pin is reproducible and checkable at start-up. The single feature
/// reads each task's declared position and abstains when it is absent. Two
/// more describe a tool step's call and its binding count
/// (MEDULLA-DECISION-ENGINE-R134.111.12). The last names the step kind: a
/// prompt, call, branch, join, decision or approval (R134.111.17).
pub fn task_plan_feature_schema() -> super::statistical::features::FeatureSchemaBody {
    use super::statistical::features::{
        FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
    };
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(
            [
                "task_position",
                "task_call",
                "task_call_bindings",
                "task_step_kind",
            ]
            .into_iter()
            .map(|name| FeatureSpec {
                name: name.to_string(),
                kind: FeatureKind::Number {
                    key: name.to_string(),
                },
                missing: MissingValue::Abstain,
            })
            .collect(),
        )
        .expect("four features are within the schema bound"),
    }
}
pub const MAX_TASKS: u16 = 64;
pub const MAX_CONCURRENCY: u16 = 8;
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;
pub const MAX_TOTAL_PROMPT_BYTES: usize = 64 * 1024;

/// A prompt step uses the host's already-bound agent. It cannot select
/// credentials, models or a different principal. A task with `call` is a tool
/// step: no model runs it and `prompt` is only its label. The call is a caller
/// claim and grants no authority to invoke the tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlanTask {
    pub task_id: String,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: BoundedVec<String, 64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<TaskPlanCall>,
    /// How the output task combines its dependency outputs (R134.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregation: Option<TaskPlanAggregation>,
    /// The component id of the chosen skill or specialist (R134.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// A branch step: its arms become ready when their condition holds
    /// (R134.111.14). At most one of `call`, `branch`, `join`, `decision` and
    /// `approval` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<BranchSpec>,
    /// A join step that waits on every arm path of a branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinSpec>,
    /// A decision step naming the decision the host asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionSpec>,
    /// An approval wait with a role and a timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalSpec>,
    /// The step's expected duration in milliseconds, positive when present
    /// (R134.15). When any task carries a duration or a priority weight, every
    /// task carries both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
    /// The step's priority weight in units of `task_plan.weight_scale` (R134.15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_weight: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlanProposal {
    pub tasks: BoundedVec<TaskPlanTask, 64>,
    pub output_task_id: String,
    pub max_concurrency: u16,
    pub max_steps: u16,
}

/// Canonical task order is a topological order with lexical ready-task ties.
/// The input digest retains the submitted order; the plan digest binds the
/// canonical executable body and policy, independent of declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlan {
    pub tasks: BoundedVec<TaskPlanTask, 64>,
    pub output_task_id: String,
    pub max_concurrency: u16,
    pub max_steps: u16,
    pub total_tasks: u16,
    pub policy_revision: String,
    pub policy_digest: String,
    pub input_digest: String,
    pub plan_digest: String,
    /// The ranked skill tree of each subtask of a `skill_query` plan
    /// (MEDULLA-DECISION-ENGINE-R134.9); absent for every other plan, whose
    /// bytes stay unchanged.
    #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
    pub skill_trees: BoundedVec<skill_tree::TaskSkillTree, 64>,
    /// The weighted critical chain, present when the tasks carry durations and
    /// priority weights (R134.15). The plan digest covers it when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_chain: Option<CriticalChain>,
    /// The horizon partition (R134.11), present on a served plan. A plan within
    /// the horizon limit has one horizon. The plan digest covers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizons: Option<horizon::HorizonPartition>,
}

/// The policy digest includes every engine bound and ordering rule.
pub fn task_plan_policy_digest() -> String {
    digest_text(
        "eg/task-plan-policy/v1",
        &(
            TASK_PLAN_POLICY_REVISION,
            MAX_TASKS,
            MAX_CONCURRENCY,
            MAX_PROMPT_BYTES,
            MAX_TOTAL_PROMPT_BYTES,
            (
                MAX_TOOL_STEPS,
                MAX_BINDINGS_PER_CALL,
                MAX_CALL_ARGUMENT_BYTES,
                MAX_BINDING_PATH_SEGMENTS,
                MAX_BINDING_PATH_BYTES,
            ),
            "lexical_ready_ids;sorted_dependencies;single_output_sink;no_retries;\
             canonical_calls;direct_dependency_bindings;no_inferred_edges;\
             aggregation_on_output_only;reducer_iff_reduce;explicit_assignee",
        ),
    )
}

/// A `task_plan` request carries zero or more than one proposer parameter
/// (MEDULLA-DECISION-ENGINE-R134.5).
pub const PLAN_PARAMETER_CONFLICT: &str = "PLAN_PARAMETER_CONFLICT";

/// The `task_plan` family's refusal table (MEDULLA-DECISION-ENGINE-R134.6): every
/// code the planner and its proposers raise, upper snake on the wire.
pub const TASK_PLAN_REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("EMPTY_PLAN"),
    RefusalCode::new("PLAN_BOUNDS"),
    RefusalCode::new("INVALID_TASK_ID"),
    RefusalCode::new("PLAN_TEXT_BOUNDS"),
    RefusalCode::new("DUPLICATE_DEPENDENCY"),
    RefusalCode::new("MISSING_DEPENDENCY"),
    RefusalCode::new("DUPLICATE_TASK_ID"),
    RefusalCode::new("PLAN_CYCLE"),
    RefusalCode::new("INVALID_OUTPUT_TASK"),
    RefusalCode::new("DISCONNECTED_OUTPUT"),
    RefusalCode::new("CALL_BOUNDS"),
    RefusalCode::new("CALL_INVALID"),
    RefusalCode::new("BINDING_BAD_PATH"),
    RefusalCode::new("BINDING_UNKNOWN_TASK"),
    RefusalCode::new("BINDING_CYCLE"),
    RefusalCode::new("BINDING_NOT_DEPENDENCY"),
    RefusalCode::new(skill_plan::NO_SKILL_FOR_SUBTASK),
    RefusalCode::new(NO_SPECIALIST_FOR_SUBTASK),
    RefusalCode::new(skill_plan::EMPTY_SKILL_INDEX),
    RefusalCode::new(PLAN_AGGREGATION_NOT_OUTPUT),
    RefusalCode::new(PLAN_AGGREGATION_INVALID),
    RefusalCode::new(PLAN_PARAMETER_CONFLICT),
    RefusalCode::new(CRITICAL_PATH_WEIGHT_MISSING),
    RefusalCode::new(CRITICAL_PATH_INVALID_DURATION),
    RefusalCode::new(CRITICAL_PATH_OVERFLOW),
    RefusalCode::new(horizon::HORIZON_UNSPLITTABLE),
];

fn error(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

fn validate_plan_bounds(proposal: &TaskPlanProposal) -> Result<(), String> {
    if proposal.tasks.is_empty() {
        return Err(error("EMPTY_PLAN", "at least one task is required"));
    }
    if proposal.max_steps == 0
        || proposal.max_steps > MAX_TASKS
        || proposal.tasks.len() > usize::from(proposal.max_steps)
        || proposal.max_concurrency == 0
        || proposal.max_concurrency > MAX_CONCURRENCY.min(proposal.max_steps)
    {
        return Err(error("PLAN_BOUNDS", "invalid task or concurrency ceiling"));
    }
    Ok(())
}

fn canonical_task(task: &TaskPlanTask, bytes: &mut usize) -> Result<TaskPlanTask, String> {
    if !valid_id(&task.task_id) {
        return Err(error("INVALID_TASK_ID", &task.task_id));
    }
    *bytes += task.prompt.len(); // at most 64 bounded prompt lengths below
    if task.prompt.trim().is_empty()
        || task.prompt.len() > MAX_PROMPT_BYTES
        || *bytes > MAX_TOTAL_PROMPT_BYTES
    {
        return Err(error("PLAN_TEXT_BOUNDS", &task.task_id));
    }
    control::check_kind(task)?;
    let mut canonical = task.clone();
    control::canonicalize(task, &mut canonical)?;
    if let Some(raw) = &task.call {
        let (call, _, call_bytes) = call::canonical_call(&task.task_id, raw)?;
        *bytes += call_bytes;
        if *bytes > MAX_TOTAL_PROMPT_BYTES {
            return Err(error("PLAN_TEXT_BOUNDS", &task.task_id));
        }
        canonical.call = Some(call);
    }
    let mut deps = task.depends_on.as_slice().to_vec();
    deps.sort();
    if deps.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error("DUPLICATE_DEPENDENCY", &task.task_id));
    }
    canonical.depends_on = BoundedVec::new(deps).expect("same bounded length");
    Ok(canonical)
}

fn validate_dependencies(tasks: &BTreeMap<String, TaskPlanTask>) -> Result<(), String> {
    for task in tasks.values() {
        for dependency in task.depends_on.iter() {
            if !tasks.contains_key(dependency) {
                return Err(error("MISSING_DEPENDENCY", dependency));
            }
        }
    }
    Ok(())
}

fn index(proposal: &TaskPlanProposal) -> Result<BTreeMap<String, TaskPlanTask>, String> {
    validate_plan_bounds(proposal)?;
    let mut tasks = BTreeMap::new();
    let mut bytes = 0usize;
    for task in proposal.tasks.iter() {
        let canonical = canonical_task(task, &mut bytes)?;
        if tasks.insert(task.task_id.clone(), canonical).is_some() {
            return Err(error("DUPLICATE_TASK_ID", &task.task_id));
        }
    }
    validate_dependencies(&tasks)?;
    Ok(tasks)
}

fn ordered(tasks: &BTreeMap<String, TaskPlanTask>) -> Result<Vec<TaskPlanTask>, String> {
    let mut finished = BTreeSet::new();
    let mut output = Vec::with_capacity(tasks.len());
    while output.len() < tasks.len() {
        let next = tasks.values().find(|task| {
            !finished.contains(&task.task_id)
                && task.depends_on.iter().all(|id| finished.contains(id))
        });
        let Some(task) = next else {
            return Err(error("PLAN_CYCLE", "dependency graph is cyclic"));
        };
        finished.insert(task.task_id.clone());
        output.push(task.clone());
    }
    Ok(output)
}

fn check_output(tasks: &BTreeMap<String, TaskPlanTask>, output: &str) -> Result<(), String> {
    if !tasks.contains_key(output) {
        return Err(error("INVALID_OUTPUT_TASK", output));
    }
    let mut covered = BTreeSet::new();
    let mut pending = vec![output];
    while let Some(id) = pending.pop() {
        if covered.insert(id) {
            pending.extend(tasks[id].depends_on.iter().map(String::as_str));
        }
    }
    if covered.len() != tasks.len() {
        return Err(error(
            "DISCONNECTED_OUTPUT",
            "every task must feed the output sink",
        ));
    }
    Ok(())
}

/// Derive a bounded executable DAG from explicit caller claims. No model,
/// external service, mutable state or authorization mechanism is consulted.
pub fn plan_explicit_tasks(proposal: &TaskPlanProposal) -> Result<TaskPlan, String> {
    let tasks = index(proposal)?;
    let canonical = ordered(&tasks)?;
    call::check_bindings(&tasks)?;
    control::check_control(&tasks)?;
    check_output(&tasks, &proposal.output_task_id)?;
    aggregation::check_aggregation(&tasks, &proposal.output_task_id)?;
    let mut plan = TaskPlan {
        total_tasks: canonical.len() as u16,
        tasks: BoundedVec::new(canonical).expect("validated task bound"),
        output_task_id: proposal.output_task_id.clone(),
        max_concurrency: proposal.max_concurrency,
        max_steps: proposal.max_steps,
        policy_revision: TASK_PLAN_POLICY_REVISION.to_string(),
        policy_digest: task_plan_policy_digest(),
        input_digest: digest_text("eg/task-plan-input/v1", proposal),
        plan_digest: String::new(),
        skill_trees: BoundedVec::default(),
        critical_chain: None,
        horizons: None,
    };
    plan.plan_digest = compute_plan_digest(&plan);
    Ok(plan)
}

/// The plan digest: the canonical body and policy, plus the critical chain when
/// the plan carries one (R134.15). A plan without a chain digests as before.
pub fn compute_plan_digest(plan: &TaskPlan) -> String {
    let body = (
        &plan.tasks,
        &plan.output_task_id,
        plan.max_concurrency,
        plan.max_steps,
        plan.total_tasks,
        &plan.policy_revision,
        &plan.policy_digest,
    );
    if plan.critical_chain.is_none() && plan.horizons.is_none() {
        digest_text("eg/task-plan/v1", &body)
    } else {
        digest_text(
            "eg/task-plan/v2",
            &(body, &plan.critical_chain, &plan.horizons),
        )
    }
}

/// The plan as `plan_explicit_tasks` derives it, without the engine-derived
/// critical chain and horizons a served plan adds.
pub fn without_derived(plan: &TaskPlan) -> TaskPlan {
    let mut base = plan.clone();
    base.critical_chain = None;
    base.horizons = None;
    base.plan_digest = compute_plan_digest(&base);
    base
}

#[cfg(test)]
mod tests;
