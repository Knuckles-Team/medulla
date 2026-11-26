//! Workflow to task plan (MEDULLA-CQ-ORCH-R704.4).
//!
//! One canonical workflow and its step bindings become one task plan: each step except
//! `start` and `end` is one task whose `depends_on` come from its incoming flows. A `call`
//! step is inlined (its callee's steps carry the id `<call step>:<callee step>`), a `boundary`
//! step becomes a branch after its activity, and the caller passes the proposal through
//! `plan_explicit_tasks`. Nothing here names a source format (R704.9); a flow condition is free
//! text, so it becomes a typed `Decision` arm condition (`flowcondition:<workflow digest>:<flow
//! id>`) that the decision engine resolves at run time; the questions are returned beside the
//! proposal so the caller can register them. No condition text is parsed or evaluated here.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::process_plan::{
    BindingTarget, ProcessPlanRefusal, StepBinding, PROCESS_PLAN_CALL_DEPTH, PROCESS_PLAN_LOOP,
    PROCESS_PLAN_TOO_LARGE, PROCESS_STEP_UNBOUND, WORKFLOW_UNKNOWN,
};
use medulla_decision_types::decision::task_plan::{
    ApprovalSpec, BranchArm, BranchCondition, BranchDecisionCondition,
    BranchKind, BranchSpec, DecisionSpec, JoinSpec, TaskPlanCall, TaskPlanProposal,
    TaskPlanTask, MAX_CONCURRENCY, MAX_DECISION_IRI_BYTES, MAX_TASKS,
};
use medulla_types::contract::BoundedVec;
use medulla_types::process_import::{
    workflow_digest_hex, CanonicalStep, CanonicalWorkflow, GateKind, StepKind,
};

/// The prefix of the decision IRI a flow condition becomes; the digest of the workflow that
/// holds the flow and the flow id (`f<n>`, the 1-based position of the flow) follow.
pub const CONDITION_IRI_PREFIX: &str = "flowcondition:";

/// One flow condition as a decision question: its IRI and the condition text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionQuestion {
    pub decision_iri: String,
    pub text: String,
}

/// The proposal of one workflow with the decision questions its flow conditions became.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanFromWorkflow {
    pub proposal: TaskPlanProposal,
    pub questions: Vec<ConditionQuestion>,
}

/// A stored workflow a `call` step names, with its explicit and ranked bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callee {
    pub workflow: CanonicalWorkflow,
    pub bindings: Vec<StepBinding>,
}

/// The stored workflow named by a `call` step's reference, if it is visible.
pub type CalleeOf<'a> = &'a dyn Fn(&str) -> Option<Callee>;

#[derive(Debug, Clone)]
struct Node {
    id: String,
    step: CanonicalStep,
    target: Option<BindingTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edge {
    source: String,
    target: String,
    condition: Option<ConditionQuestion>,
    default: bool,
}

#[derive(Debug, Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

fn refusal(code: &str, steps: Vec<String>) -> ProcessPlanRefusal {
    ProcessPlanRefusal {
        code: code.to_string(),
        steps,
    }
}

fn is_anchor(kind: StepKind) -> bool {
    matches!(kind, StepKind::Start | StepKind::End)
}

/// The nodes and edges of `workflow` with ids prefixed by `prefix`, every `call` step replaced
/// by its callee's graph. `depth` is the nesting level of `workflow` (the root is 0).
fn flatten(
    workflow: &CanonicalWorkflow,
    bindings: &[StepBinding],
    prefix: &str,
    depth: u32,
    max_depth: u32,
    callee: CalleeOf<'_>,
) -> Result<Graph, ProcessPlanRefusal> {
    let bound: BTreeMap<&str, &BindingTarget> = bindings
        .iter()
        .map(|b| (b.step_id.as_str(), &b.target))
        .collect();
    let mut graph = Graph::default();
    // For each call step: the entry and exit nodes of its inlined callee.
    let mut calls: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for step in &workflow.steps {
        let id = format!("{prefix}{}", step.id);
        if step.kind != StepKind::Call {
            graph.nodes.push(Node {
                id,
                step: step.clone(),
                target: bound.get(step.id.as_str()).map(|t| (*t).clone()),
            });
            continue;
        }
        if depth + 1 > max_depth {
            return Err(refusal(PROCESS_PLAN_CALL_DEPTH, vec![id]));
        }
        let called = step
            .reference
            .as_deref()
            .and_then(callee)
            .ok_or_else(|| refusal(WORKFLOW_UNKNOWN, vec![id.clone()]))?;
        let inner = flatten(
            &called.workflow,
            &called.bindings,
            &format!("{id}:"),
            depth + 1,
            max_depth,
            callee,
        )?;
        let has_in: BTreeSet<&str> = inner.edges.iter().map(|e| e.target.as_str()).collect();
        let has_out: BTreeSet<&str> = inner.edges.iter().map(|e| e.source.as_str()).collect();
        let pick = |anchor: StepKind, linked: &BTreeSet<&str>| -> Vec<String> {
            let anchors: Vec<String> = inner
                .nodes
                .iter()
                .filter(|n| n.step.kind == anchor)
                .map(|n| n.id.clone())
                .collect();
            if !anchors.is_empty() {
                return anchors;
            }
            inner
                .nodes
                .iter()
                .filter(|n| !linked.contains(n.id.as_str()))
                .map(|n| n.id.clone())
                .collect()
        };
        let entries = pick(StepKind::Start, &has_in);
        let exits = pick(StepKind::End, &has_out);
        calls.insert(id, (entries, exits));
        graph.nodes.extend(inner.nodes);
        graph.edges.extend(inner.edges);
    }
    for (position, flow) in workflow.flows.iter().enumerate() {
        let source = format!("{prefix}{}", flow.source);
        let target = format!("{prefix}{}", flow.target);
        let sources = calls
            .get(&source)
            .map_or_else(|| vec![source.clone()], |(_, exits)| exits.clone());
        let targets = calls
            .get(&target)
            .map_or_else(|| vec![target.clone()], |(entries, _)| entries.clone());
        for source in &sources {
            for target in &targets {
                graph.edges.push(Edge {
                    source: source.clone(),
                    target: target.clone(),
                    condition: flow.condition.as_ref().map(|text| ConditionQuestion {
                        decision_iri: format!(
                            "{CONDITION_IRI_PREFIX}{}:f{}",
                            workflow_digest_hex(&workflow.digest),
                            position + 1
                        ),
                        text: text.clone(),
                    }),
                    default: flow.default,
                });
            }
        }
    }
    Ok(graph)
}

/// Remove every `start` and `end` node, joining each one's predecessors to its successors so a
/// dependency through an inlined callee's anchors survives.
fn contract_anchors(graph: &mut Graph) {
    let anchors: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| is_anchor(n.step.kind))
        .map(|n| n.id.clone())
        .collect();
    for anchor in anchors {
        let (touching, rest): (Vec<Edge>, Vec<Edge>) = std::mem::take(&mut graph.edges)
            .into_iter()
            .partition(|e| e.source == anchor || e.target == anchor);
        graph.edges = rest;
        let incoming: Vec<&Edge> = touching.iter().filter(|e| e.target == anchor).collect();
        let outgoing: Vec<&Edge> = touching.iter().filter(|e| e.source == anchor).collect();
        for inbound in &incoming {
            for outbound in &outgoing {
                if inbound.source == anchor || outbound.target == anchor {
                    continue;
                }
                graph.edges.push(Edge {
                    source: inbound.source.clone(),
                    target: outbound.target.clone(),
                    condition: inbound.condition.clone().or_else(|| outbound.condition.clone()),
                    default: inbound.default || outbound.default,
                });
            }
        }
        graph.nodes.retain(|n| n.id != anchor);
    }
}

/// A boundary event is reached from the activity it attaches to. Each other flow out of that
/// activity moves to leave the boundary instead (the normal arm), and each flow out of the
/// boundary without a condition of its own carries the trigger as its condition.
fn lower_boundaries(graph: &mut Graph, digest: &str) {
    let mut boundaries: Vec<(String, String)> = graph
        .nodes
        .iter()
        .filter(|n| n.step.kind == StepKind::Boundary)
        .map(|n| {
            let trigger = n.step.reference.clone().unwrap_or_else(|| n.step.label.clone());
            (n.id.clone(), trigger)
        })
        .collect();
    // With several boundaries on one activity the first by id carries the normal arms.
    boundaries.sort();
    let boundary_ids: BTreeSet<String> = boundaries.iter().map(|(id, _)| id.clone()).collect();
    for (boundary, trigger) in &boundaries {
        let hosts: Vec<String> = graph
            .edges
            .iter()
            .filter(|e| &e.target == boundary && !boundary_ids.contains(&e.source))
            .map(|e| e.source.clone())
            .collect();
        for edge in &mut graph.edges {
            if hosts.contains(&edge.source) && !boundary_ids.contains(&edge.target) {
                edge.source = boundary.clone();
            } else if &edge.source == boundary && edge.condition.is_none() && !edge.default {
                edge.condition = Some(ConditionQuestion {
                    decision_iri: format!("{CONDITION_IRI_PREFIX}{digest}:{boundary}:trigger"),
                    text: format!("trigger {trigger}"),
                });
            }
        }
    }
}

/// The step ids on a cycle: what is left when no step without a predecessor remains. Returns
/// the topological order when there is none.
fn topological(graph: &Graph) -> Result<Vec<String>, Vec<String>> {
    let mut indegree: BTreeMap<&str, usize> =
        graph.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    for edge in &graph.edges {
        if let Some(count) = indegree.get_mut(edge.target.as_str()) {
            *count += 1;
        }
    }
    let position: BTreeMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut ready: BTreeSet<(usize, &str)> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| (position[id], *id))
        .collect();
    let mut order = Vec::new();
    while let Some((_, id)) = ready.pop_first() {
        order.push(id.to_string());
        for edge in graph.edges.iter().filter(|e| e.source == id) {
            if let Some(count) = indegree.get_mut(edge.target.as_str()) {
                *count -= 1;
                if *count == 0 {
                    ready.insert((position[edge.target.as_str()], edge.target.as_str()));
                }
            }
        }
    }
    if order.len() == graph.nodes.len() {
        return Ok(order);
    }
    let done: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    Err(graph
        .nodes
        .iter()
        .filter(|n| !done.contains(n.id.as_str()))
        .map(|n| n.id.clone())
        .collect())
}

/// The gate class of a split or a join: a `branch` is exclusive and a `fork` parallel unless
/// its gate says otherwise.
fn split_gate(step: &CanonicalStep) -> Option<GateKind> {
    match step.kind {
        StepKind::Branch => Some(step.gate.unwrap_or(GateKind::Xor)),
        StepKind::Fork => Some(step.gate.unwrap_or(GateKind::And)),
        _ => None,
    }
}

/// For each join, the nearest split that dominates it with the same gate.
fn join_splits(graph: &Graph, order: &[String]) -> BTreeMap<String, String> {
    let nodes: BTreeMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut dominators: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for id in order {
        let id = id.as_str();
        let mut preds = graph
            .edges
            .iter()
            .filter(|e| e.target == id)
            .filter_map(|e| dominators.get(e.source.as_str()));
        let mut set: BTreeSet<&str> = match preds.next() {
            Some(first) => first.clone(),
            None => BTreeSet::new(),
        };
        for next in preds {
            set.retain(|d| next.contains(d));
        }
        set.insert(id);
        dominators.insert(id, set);
    }
    let mut out = BTreeMap::new();
    for node in &graph.nodes {
        if node.step.kind != StepKind::Join {
            continue;
        }
        let want = node.step.gate.unwrap_or(GateKind::And);
        let nearest = dominators[node.id.as_str()]
            .iter()
            .copied()
            .filter(|d| *d != node.id.as_str())
            .filter(|d| split_gate(&nodes[*d].step) == Some(want))
            .max_by_key(|d| dominators[*d].len());
        if let Some(split) = nearest {
            out.insert(node.id.clone(), split.to_string());
        }
    }
    out
}

fn arm_condition(question: &ConditionQuestion) -> Result<BranchCondition, ProcessPlanRefusal> {
    if question.decision_iri.len() > MAX_DECISION_IRI_BYTES {
        return Err(refusal(PROCESS_PLAN_TOO_LARGE, Vec::new()));
    }
    Ok(BranchCondition::Decision(BranchDecisionCondition {
        decision_iri: question.decision_iri.clone(),
    }))
}

fn branch_of(
    node: &Node,
    graph: &Graph,
    questions: &mut Vec<ConditionQuestion>,
) -> Result<BranchSpec, ProcessPlanRefusal> {
    let kind = match (node.step.kind, node.step.gate) {
        (StepKind::Fork, Some(GateKind::Or)) => BranchKind::Inclusive,
        (StepKind::Fork, Some(GateKind::And) | None) => BranchKind::Parallel,
        _ => BranchKind::Exclusive,
    };
    let mut outgoing: Vec<&Edge> = graph.edges.iter().filter(|e| e.source == node.id).collect();
    outgoing.sort_by(|a, b| a.target.cmp(&b.target));
    let mut default_arm = None;
    let mut arms = Vec::new();
    for edge in outgoing {
        if edge.default && default_arm.is_none() {
            default_arm = Some(edge.target.clone());
            continue;
        }
        let condition = match (&edge.condition, kind) {
            (Some(question), BranchKind::Exclusive | BranchKind::Inclusive) => {
                let condition = arm_condition(question)?;
                if !questions.contains(question) {
                    questions.push(question.clone());
                }
                Some(condition)
            }
            _ => None,
        };
        arms.push(BranchArm {
            target_task_id: edge.target.clone(),
            condition,
        });
    }
    Ok(BranchSpec {
        kind,
        arms: BoundedVec::new(arms)
            .map_err(|_| refusal(PROCESS_PLAN_TOO_LARGE, vec![node.id.clone()]))?,
        exhaustive: kind != BranchKind::Parallel && default_arm.is_none(),
        default_arm,
    })
}

fn task_of(
    node: &Node,
    graph: &Graph,
    splits: &BTreeMap<String, String>,
    questions: &mut Vec<ConditionQuestion>,
    approval_timeout_s: u64,
) -> Result<Option<TaskPlanTask>, ProcessPlanRefusal> {
    let kind = node.step.kind;
    let mut depends: Vec<String> = graph
        .edges
        .iter()
        .filter(|e| e.target == node.id)
        .map(|e| e.source.clone())
        .collect();
    depends.sort();
    depends.dedup();
    let label = node.step.label.trim();
    let mut task = TaskPlanTask {
        task_id: node.id.clone(),
        prompt: if label.is_empty() {
            node.id.clone()
        } else {
            label.to_string()
        },
        depends_on: BoundedVec::new(depends)
            .map_err(|_| refusal(PROCESS_PLAN_TOO_LARGE, vec![node.id.clone()]))?,
        call: None,
        aggregation: None,
        assignee: None,
        branch: None,
        join: None,
        decision: None,
        approval: None,
        duration_ms: None,
        priority_weight: None,
        foreach: None,
    };
    match (kind, &node.target) {
        (StepKind::Agent, Some(BindingTarget::Tool { server, tool, .. })) => {
            task.call = Some(TaskPlanCall {
                server: server.clone(),
                tool: tool.clone(),
                arguments: serde_json::json!({}),
            });
        }
        (StepKind::Agent, Some(BindingTarget::Prompt { .. })) => {}
        (StepKind::Decision, Some(BindingTarget::Decision { decision_iri })) => {
            task.decision = Some(DecisionSpec {
                decision_iri: decision_iri.clone(),
                inputs: BoundedVec::default(),
            });
        }
        (StepKind::Human | StepKind::Wait, Some(BindingTarget::Approval { approver_role })) => {
            task.approval = Some(ApprovalSpec {
                approver_role: approver_role.clone(),
                timeout_s: approval_timeout_s,
            });
        }
        (StepKind::Branch | StepKind::Fork | StepKind::Boundary, _) => {
            task.branch = Some(branch_of(node, graph, questions)?);
        }
        (StepKind::Join, _) => {
            let Some(split) = splits.get(&node.id) else {
                return Err(refusal(
                    "BRANCH_JOIN_UNMATCHED",
                    vec![node.id.clone()],
                ));
            };
            task.join = Some(JoinSpec {
                branch_task_id: split.clone(),
            });
        }
        (StepKind::Start | StepKind::End, _) => return Ok(None),
        // An agent, decision, human or wait step with no fitting binding, or a `call` step
        // that was not inlined.
        _ => return Err(refusal(PROCESS_STEP_UNBOUND, vec![node.id.clone()])),
    }
    Ok(Some(task))
}

/// The proposal for `workflow` and its `bindings`, with the decision questions its flow
/// conditions became. `callee` resolves the workflow a `call` step names; `max_call_depth` is
/// `knob process_plan.max_call_depth`; `approval_timeout_s` is `knob
/// process_plan.approval_timeout_s`, the seconds a human or wait approval lasts. The caller passes the proposal through
/// `plan_explicit_tasks`.
pub fn plan_from_workflow(
    workflow: &CanonicalWorkflow,
    bindings: &[StepBinding],
    callee: CalleeOf<'_>,
    max_call_depth: u32,
    approval_timeout_s: u64,
) -> Result<PlanFromWorkflow, ProcessPlanRefusal> {
    let mut graph = flatten(workflow, bindings, "", 0, max_call_depth, callee)?;
    contract_anchors(&mut graph);
    lower_boundaries(&mut graph, workflow_digest_hex(&workflow.digest));
    let order = match topological(&graph) {
        Ok(order) => order,
        Err(cycle) => {
            let gates: Vec<String> = graph
                .nodes
                .iter()
                .filter(|n| cycle.contains(&n.id))
                .filter(|n| {
                    matches!(
                        n.step.kind,
                        StepKind::Branch | StepKind::Fork | StepKind::Join | StepKind::Boundary
                    )
                })
                .map(|n| n.id.clone())
                .collect();
            return Err(refusal(
                PROCESS_PLAN_LOOP,
                if gates.is_empty() { cycle } else { gates },
            ));
        }
    };
    if graph.nodes.len() > usize::from(MAX_TASKS) {
        return Err(refusal(PROCESS_PLAN_TOO_LARGE, Vec::new()));
    }
    let splits = join_splits(&graph, &order);
    let mut tasks = Vec::new();
    let mut unbound = Vec::new();
    let mut questions = Vec::new();
    for node in &graph.nodes {
        match task_of(node, &graph, &splits, &mut questions, approval_timeout_s) {
            Ok(task) => tasks.extend(task),
            Err(refused) if refused.code == PROCESS_STEP_UNBOUND => unbound.extend(refused.steps),
            Err(refused) => return Err(refused),
        }
    }
    if !unbound.is_empty() {
        return Err(refusal(PROCESS_STEP_UNBOUND, unbound));
    }
    let sources: BTreeSet<&str> = graph.edges.iter().map(|e| e.source.as_str()).collect();
    let output = order
        .iter()
        .rev()
        .find(|id| !sources.contains(id.as_str()))
        .cloned()
        .unwrap_or_default();
    Ok(PlanFromWorkflow {
        proposal: TaskPlanProposal {
            tasks: BoundedVec::new(tasks)
                .map_err(|_| refusal(PROCESS_PLAN_TOO_LARGE, Vec::new()))?,
            output_task_id: output,
            max_concurrency: MAX_CONCURRENCY,
            max_steps: MAX_TASKS,
            final_artifact: None,
        },
        questions,
    })
}

/// The refusal of the task plan engine, as `ProcessPlanRefusal`: the message is
/// `<CODE>: <detail>`; the refusal keeps the code and names no step.
pub fn engine_refusal(message: &str) -> ProcessPlanRefusal {
    ProcessPlanRefusal {
        code: message.split(':').next().unwrap_or_default().to_string(),
        steps: Vec::new(),
    }
}
