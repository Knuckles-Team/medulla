//! The `process_plan` question family (MEDULLA-CQ-ORCH-R704.5).
//!
//! Plans one stored canonical workflow: the stored `WorkflowPattern` version is read from the
//! graph named by the `graph` param, each step is bound from its graph links
//! (`explicit_bindings`), an unbound agent step falls back to tool relevance
//! (`ranked_bindings`), and `plan_from_workflow` seals the task plan through
//! `plan_explicit_tasks`. Evaluate-only: nothing is written. The plan reads the canonical
//! workflow and the binding links of its step nodes only, never a source format (R704.9).

use std::cell::RefCell;

use medulla_compute_assemble::process_plan::rank::ranked_bindings;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::process_plan::{
    BindingSource, BindingTarget, ProcessPlanKnobs, ProcessPlanRefusal, ProcessPlanRequest,
    StepBinding, PROCESS_PLAN_REFUSALS,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::task_plan::plan_explicit_tasks;
use medulla_decision_types::decision::tool_relevance::{
    AssignedTool, ToolAssignmentRequest, MAX_TOP_K, TOOL_RELEVANCE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::process_import::{workflow_node_id, CanonicalWorkflow};
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_compute_assemble::process_plan::bind::registered_contract,
    medulla_compute_assemble::process_plan::bind::resolve_bindings,
};
use {
    medulla_compute_assemble::process_plan::plan::engine_refusal,
    medulla_compute_assemble::process_plan::plan::plan_from_workflow,
    medulla_compute_assemble::process_plan::plan::Callee,
    medulla_compute_assemble::process_plan::plan::ConditionQuestion,
};

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_task_plan::{PlanNotes, Planned};
use crate::decide::SharedState;

use super::support::{invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The parameter naming the graph that holds the stored workflow.
pub const PARAM_GRAPH: &str = "graph";
/// The parameter carrying the JSON `ProcessPlanRequest`.
pub const PARAM_REQUEST: &str = "process_plan";
/// The one declared option of a process plan question.
pub const PLAN_OPTION: &str = "process_plan";

/// The family of the `process_plan` question kind.
pub struct ProcessPlanFamily;

/// The graph a process plan question reads its stored workflow from, under the caller's read
/// ACL and RLS filter.
pub(in crate::decide) async fn evidence_view(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match text_param(request, PARAM_GRAPH)? {
        Some(graph) => crate::decide::stat_support::filtered_view(state, agent_id, graph)
            .await
            .map(Some),
        None => Err(invalid(format!(
            "a process_plan question requires the {PARAM_GRAPH} parameter"
        ))),
    }
}

/// The stored workflow `workflow_id` at `version` that `tenant` may see.
fn stored_workflow(
    view: &GraphView,
    tenant: &str,
    workflow_id: &str,
    version: u32,
) -> Option<(String, CanonicalWorkflow)> {
    let node = workflow_node_id(workflow_id, version);
    let properties = view.node_row_object(&node)?;
    let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
    if text("type") != Some("WorkflowPattern") || text("tenant_id").is_some_and(|t| t != tenant) {
        return None;
    }
    let workflow = serde_json::from_str(text("canonical")?).ok()?;
    Some((node, workflow))
}

/// The newest stored version of the workflow `workflow_id` in `view`: 0 when none is stored.
fn latest_version(view: &GraphView, workflow_id: &str) -> u32 {
    let mut version = 0u32;
    while view
        .node_map
        .contains_key(&workflow_node_id(workflow_id, version + 1))
    {
        version += 1;
    }
    version
}

/// Binds the steps of stored workflows: explicit links first, then the ranked fallback. A
/// refusal of the tool relevance read is kept and surfaces after the plan is built, never
/// swallowed.
struct Binder<'a> {
    ctx: &'a ExecutionContext<'a>,
    policy: &'a DecisionPolicy,
    view: &'a GraphView,
    knobs: ProcessPlanKnobs,
    failure: RefCell<Option<String>>,
}

impl Binder<'_> {
    fn relevance(&self, label: &str, lane: &str) -> Vec<AssignedTool> {
        let asked = ToolAssignmentRequest {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            task_text: label.to_string(),
            task_embedding: None,
            agent_role_iri: lane.to_string(),
            top_k: MAX_TOP_K,
            context_budget_tokens: u32::MAX,
            required_capabilities: BoundedVec::default(),
        };
        match super::tool_relevance::fused_ranking(self.ctx, self.policy, &asked) {
            Ok((_, ranked)) => ranked,
            Err(detail) => {
                self.failure.borrow_mut().get_or_insert(detail);
                Vec::new()
            }
        }
    }

    /// The explicit bindings of `workflow` (stored at `node`) and the ranked bindings of its
    /// unbound agent steps.
    fn bind(&self, node: &str, workflow: &CanonicalWorkflow) -> (Vec<StepBinding>, Vec<String>) {
        let explicit = resolve_bindings(self.view, node, workflow);
        let contract_of = |server: &str, tool: &str| registered_contract(self.view, server, tool);
        let ranked = ranked_bindings(
            &workflow.steps,
            &explicit.bindings,
            self.knobs.min_tool_score_micros,
            &|label, lane| self.relevance(label, lane),
            &contract_of,
        );
        let mut bindings = explicit.bindings;
        bindings.extend(ranked);
        (bindings, explicit.premises)
    }
}

fn refused(request: &ProcessPlanRequest, refusal: ProcessPlanRefusal) -> Result<Planned, String> {
    let Some(code) = PROCESS_PLAN_REFUSALS
        .iter()
        .find(|code| code.as_str() == refusal.code)
        .copied()
    else {
        // A refusal of the task plan engine keeps its own code.
        return Err(format!(
            "{}: workflow={}",
            refusal.code, request.workflow_id
        ));
    };
    Ok(abstain(request, code, &refusal.steps))
}

fn abstain(request: &ProcessPlanRequest, code: RefusalCode, steps: &[String]) -> Planned {
    let detail = format!("workflow={};steps={}", request.workflow_id, steps.join(","));
    Planned::Abstain {
        code,
        premise: PremiseRef {
            subject: request.workflow_id.clone(),
            fact: format!(
                "process_plan_refusal:code={};steps={}",
                code.as_str(),
                steps.join(",")
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: format!("params.{PARAM_REQUEST}"),
            },
            trust_tier: None,
        },
        detail,
    }
}

/// The kind and reference a binding names: `tool:<server>/<tool>`, `prompt:<iri>`,
/// `decision:<iri>` or `approval:<role>`.
fn target_text(target: &BindingTarget) -> String {
    match target {
        BindingTarget::Tool { server, tool, .. } => format!("tool:{server}/{tool}"),
        BindingTarget::Prompt { prompt_iri, .. } => format!("prompt:{prompt_iri}"),
        BindingTarget::Decision { decision_iri } => format!("decision:{decision_iri}"),
        BindingTarget::Approval { approver_role } => {
            format!("approval:{role}", role = approver_role)
        }
    }
}

/// The premises of one process plan (MEDULLA-CQ-ORCH-R704.7): the workflow id, version and
/// digest, one premise per step binding, and one per binding link a step named but did not
/// keep. A ranked binding is a claim; the graph's own links are observations.
fn premises(
    graph: &str,
    asked: &ProcessPlanRequest,
    workflow: &CanonicalWorkflow,
    bindings: &[StepBinding],
    dropped: &[String],
    questions: &[ConditionQuestion],
) -> Vec<PremiseRef> {
    let read = |subject: &str, fact: String, class: PremiseClass| PremiseRef {
        subject: subject.to_string(),
        fact,
        class,
        provenance: PremiseProvenance::GraphRow {
            graph: graph.to_string(),
            graph_version: 0,
        },
        trust_tier: None,
    };
    let mut out = vec![read(
        &asked.workflow_id,
        format!(
            "process_plan_workflow:id={};version={};workflow_digest={}",
            asked.workflow_id, asked.version, workflow.digest
        ),
        PremiseClass::Observation,
    )];
    for binding in bindings {
        let (source, score, class) = match &binding.source {
            BindingSource::Explicit => ("explicit", "none".to_string(), PremiseClass::Observation),
            BindingSource::Ranked { score_q32 } => {
                ("ranked", score_q32.to_string(), PremiseClass::Claim)
            }
        };
        out.push(read(
            &binding.step_id,
            format!(
                "binding:step={};target={};source={source};score_q32={score}",
                binding.step_id,
                target_text(&binding.target)
            ),
            class,
        ));
    }
    out.extend(
        dropped
            .iter()
            .map(|fact| read(&asked.workflow_id, fact.clone(), PremiseClass::Observation)),
    );
    // Each flow condition that became a decision question, for the host to register.
    out.extend(questions.iter().map(|question| {
        read(
            &question.decision_iri,
            format!(
                "flow_condition:decision_iri={};text={}",
                question.decision_iri, question.text
            ),
            PremiseClass::Observation,
        )
    }));
    out
}

/// Plan the workflow the request names.
pub(in crate::decide) fn plan(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    graph_view: Option<&GraphView>,
) -> Result<Planned, String> {
    let asked: ProcessPlanRequest = json_param(request, PARAM_REQUEST)?;
    let view = graph_view.ok_or_else(|| {
        invalid(format!(
            "a process_plan question requires the {PARAM_GRAPH} parameter"
        ))
    })?;
    let unknown = || {
        abstain(
            &asked,
            RefusalCode::new(medulla_decision_types::decision::process_plan::WORKFLOW_UNKNOWN),
            &[],
        )
    };
    let Some((node, workflow)) =
        stored_workflow(view, ctx.tenant_id, &asked.workflow_id, asked.version)
    else {
        return Ok(unknown());
    };
    let knobs = ProcessPlanKnobs::from_policy(policy);
    let binder = Binder {
        ctx,
        policy,
        view,
        knobs,
        failure: RefCell::new(None),
    };
    let (bindings, dropped) = binder.bind(&node, &workflow);
    let callee = |reference: &str| {
        let version = latest_version(view, reference);
        let (node, workflow) = stored_workflow(view, ctx.tenant_id, reference, version)?;
        let (bindings, _) = binder.bind(&node, &workflow);
        Some(Callee { workflow, bindings })
    };
    let planned = plan_from_workflow(
        &workflow,
        &bindings,
        &callee,
        knobs.max_call_depth,
        knobs.approval_timeout_s,
    )
    .and_then(|drafted| {
        plan_explicit_tasks(&drafted.proposal)
            .map(|plan| (plan, drafted.questions))
            .map_err(|message| engine_refusal(&message))
    });
    if let Some(detail) = binder.failure.take() {
        return Err(detail);
    }
    match planned {
        Ok((plan, questions)) => {
            let graph = text_param(request, PARAM_GRAPH)?.unwrap_or_default();
            let notes = PlanNotes {
                process_premises: premises(
                    graph, &asked, &workflow, &bindings, &dropped, &questions,
                ),
                process_ranked: bindings
                    .iter()
                    .any(|b| matches!(b.source, BindingSource::Ranked { .. })),
                ..PlanNotes::default()
            };
            Ok(Planned::Plan(Box::new(plan), notes))
        }
        Err(refusal) => refused(&asked, refusal),
    }
}

impl DecisionFamily for ProcessPlanFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, PLAN_OPTION)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(plan(ctx, request, policy, graph_view).map(|planned| {
            match planned {
                // Any ranked binding makes the plan a claim; bound by the graph alone it is an
                // observation.
                Planned::Plan(plan, notes) => {
                    let mut done = executed(crate::decide::stat_task_plan::outcome(&plan));
                    done.family = Some(FamilyExecuted {
                        answer: None,
                        premises: Vec::new(),
                        resolution: None,
                        evidence: Some(if notes.process_ranked {
                            EvidenceClass::Claim
                        } else {
                            EvidenceClass::Observation
                        }),
                    });
                    done
                }
                Planned::Abstain { code, detail, .. } => executed(StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::Family {
                        family: QuestionKind::ProcessPlan,
                        code,
                        detail,
                    }])
                    .expect("one reason fits the bound"),
                }),
            }
        }))
    }
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        family: None,
        family_premises: Vec::new(),
        route: None,
    }
}
