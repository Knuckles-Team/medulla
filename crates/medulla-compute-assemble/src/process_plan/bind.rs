//! Explicit step bindings from the graph (MEDULLA-CQ-ORCH-R704.2).
//!
//! A stored workflow version holds one `WorkflowStep` node per canonical step, reached from the
//! workflow node by `hasStep`. The binding links sit on that node: the `usesTool`, `hasPrompt`
//! and `evaluatesDecision` edges and the `toolArguments` and `approverRole` properties. The
//! function reads those nodes and the canonical steps only, never a source class.

use std::collections::{BTreeMap, HashMap};

use medulla_decision_types::decision::process_plan::{BindingSource, BindingTarget, StepBinding};
use medulla_graph_core::graph::GraphView;
use medulla_types::process_import::{CanonicalStep, CanonicalWorkflow, StepKind};

/// The explicit bindings of a workflow, and what the binding left out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExplicitBindings {
    /// One binding per step the graph binds, in step order.
    pub bindings: Vec<StepBinding>,
    /// One premise per binding kind a step names but did not keep, and one for each tool
    /// argument template (the binding record has no field for it).
    pub premises: Vec<String>,
}

/// The id of the stored step node of `step_id` in the workflow node `workflow_node`.
pub fn step_node_id(workflow_node: &str, step_id: &str) -> String {
    format!("{workflow_node}:step:{step_id}")
}

fn text(properties: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn decode(view: &GraphView, node: &str) -> serde_json::Map<String, serde_json::Value> {
    view.node_row_object(node).unwrap_or_default()
}

/// The display name of a node: its `name`, else its `label`, else its id.
fn name_of(view: &GraphView, node: &str) -> String {
    let properties = decode(view, node);
    text(&properties, "name")
        .or_else(|| text(&properties, "label"))
        .unwrap_or_else(|| node.to_string())
}

/// Every outgoing edge of the nodes in `sources`, by relationship: `source -> [(relationship,
/// target)]`, sorted so equal views give equal bindings.
fn links(
    view: &GraphView,
    sources: &dyn Fn(&str) -> bool,
) -> HashMap<String, Vec<(String, String)>> {
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for ((source, target), blobs) in &view.edge_properties {
        if !sources(source) {
            continue;
        }
        for blob in blobs {
            let Ok(properties) = medulla_types::msgpack::decode_property_object(blob) else {
                continue;
            };
            if let Some(relationship) = text(&properties, "relationship") {
                out.entry(source.clone())
                    .or_default()
                    .push((relationship, target.clone()));
            }
        }
    }
    for edges in out.values_mut() {
        edges.sort();
        edges.dedup();
    }
    out
}

fn target_of<'a>(edges: &'a [(String, String)], relationship: &str) -> Option<&'a str> {
    edges
        .iter()
        .find(|(name, _)| name == relationship)
        .map(|(_, target)| target.as_str())
}

/// The agent kind a lane named `role` gives: the name of the `hasAgentKind` target of the node
/// whose name is `role`.
fn agent_kind(view: &GraphView, role: &str) -> Option<String> {
    let mut lanes: Vec<&String> = view
        .node_map
        .keys()
        .filter(|id| name_of(view, id) == role)
        .collect();
    lanes.sort();
    let lane_ids: Vec<String> = lanes.into_iter().cloned().collect();
    let edges = links(view, &|source| lane_ids.iter().any(|id| id == source));
    lane_ids.iter().find_map(|lane| {
        target_of(edges.get(lane)?, "hasAgentKind").map(|kind| name_of(view, kind))
    })
}

/// The catalog tool a `usesTool` link names: the connector that serves it, its name, and the
/// IRI of its contract (the id of the catalog node).
fn tool_target(view: &GraphView, tool_node: &str) -> BindingTarget {
    let properties = decode(view, tool_node);
    let server = text(&properties, "ofConnector").unwrap_or_else(|| {
        let edges = links(view, &|source| source == tool_node);
        edges
            .get(tool_node)
            .and_then(|edges| target_of(edges, "ofConnector"))
            .map(|connector| name_of(view, connector))
            .unwrap_or_default()
    });
    BindingTarget::Tool {
        server,
        tool: name_of(view, tool_node),
        contract_iri: tool_node.to_string(),
    }
}

/// The IRI of the contract registered for the tool `tool` served by `server`: the `contractIri`
/// property of the catalog node, else the target of its `hasContract` edge. The node is the one
/// named `tool` whose `ofConnector` (property or edge) names `server`; the first by node id
/// wins. A tool with no such node, or with no registered contract, answers `None`.
pub fn registered_contract(view: &GraphView, server: &str, tool: &str) -> Option<String> {
    let mut nodes: Vec<&String> = view
        .node_map
        .keys()
        .filter(|id| {
            let properties = decode(view, id);
            text(&properties, "name").as_deref() == Some(tool)
        })
        .collect();
    nodes.sort();
    let ids: Vec<String> = nodes.into_iter().cloned().collect();
    let edges = links(view, &|source| ids.iter().any(|id| id == source));
    ids.iter().find_map(|node| {
        let properties = decode(view, node);
        let served_by = text(&properties, "ofConnector").or_else(|| {
            edges
                .get(node)
                .and_then(|edges| target_of(edges, "ofConnector"))
                .map(|connector| name_of(view, connector))
        });
        if served_by.as_deref() != Some(server) {
            return None;
        }
        text(&properties, "contractIri").or_else(|| {
            edges
                .get(node)
                .and_then(|edges| target_of(edges, "hasContract"))
                .map(str::to_string)
        })
    })
}

fn kind_name(target: &BindingTarget) -> &'static str {
    match target {
        BindingTarget::Tool { .. } => "tool",
        BindingTarget::Prompt { .. } => "prompt",
        BindingTarget::Decision { .. } => "decision",
        BindingTarget::Approval { .. } => "approval",
    }
}

/// The binding kinds `step` names, in binding order: tool, prompt, decision, approval.
fn candidates(
    view: &GraphView,
    step: &CanonicalStep,
    properties: &serde_json::Map<String, serde_json::Value>,
    edges: &[(String, String)],
) -> Vec<BindingTarget> {
    let mut out = Vec::new();
    match step.kind {
        StepKind::Agent => {
            if let Some(tool) = target_of(edges, "usesTool") {
                out.push(tool_target(view, tool));
            }
            if let Some(prompt) = target_of(edges, "hasPrompt") {
                if let Some(kind) = step.role.as_deref().and_then(|role| agent_kind(view, role)) {
                    out.push(BindingTarget::Prompt {
                        prompt_iri: prompt.to_string(),
                        agent_kind: kind,
                    });
                }
            }
        }
        StepKind::Decision => {
            let linked = target_of(edges, "evaluatesDecision").map(str::to_string);
            if let Some(decision) = step.reference.clone().or(linked) {
                out.push(BindingTarget::Decision {
                    decision_iri: decision,
                });
            }
        }
        StepKind::Human => {
            if let Some(role) = text(properties, "approverRole").or_else(|| step.role.clone()) {
                out.push(BindingTarget::Approval {
                    approver_role: role,
                });
            }
        }
        StepKind::Wait => {
            if let Some(trigger) = &step.reference {
                out.push(BindingTarget::Approval {
                    approver_role: format!("signal:{trigger}"),
                });
            }
        }
        // `call` inlines the stored workflow its reference names (the family of R704.4);
        // anchors, branches, forks, joins and boundaries run nothing.
        StepKind::Call
        | StepKind::Start
        | StepKind::End
        | StepKind::Branch
        | StepKind::Fork
        | StepKind::Join
        | StepKind::Boundary => {}
    }
    out
}

/// Bind the steps of `workflow`, the version stored at `workflow_node`, from the links on its
/// step nodes. A step with more than one binding kind keeps the first and records the rest as
/// premises.
pub fn resolve_bindings(
    view: &GraphView,
    workflow_node: &str,
    workflow: &CanonicalWorkflow,
) -> ExplicitBindings {
    let node_ids: BTreeMap<String, &CanonicalStep> = workflow
        .steps
        .iter()
        .map(|step| (step_node_id(workflow_node, &step.id), step))
        .collect();
    let edges = links(view, &|source| node_ids.contains_key(source));
    let mut out = ExplicitBindings::default();
    for step in &workflow.steps {
        let node = step_node_id(workflow_node, &step.id);
        let step_edges: &[(String, String)] = edges.get(&node).map(Vec::as_slice).unwrap_or(&[]);
        let properties = decode(view, &node);
        let mut found = candidates(view, step, &properties, step_edges).into_iter();
        let Some(first) = found.next() else {
            continue;
        };
        for dropped in found {
            out.premises.push(format!(
                "binding_superseded:step={};kept={};dropped={}",
                step.id,
                kind_name(&first),
                kind_name(&dropped)
            ));
        }
        if let (BindingTarget::Tool { .. }, Some(arguments)) =
            (&first, text(&properties, "toolArguments"))
        {
            out.premises
                .push(format!("tool_arguments:step={};{arguments}", step.id));
        }
        out.bindings.push(StepBinding {
            step_id: step.id.clone(),
            target: first,
            source: BindingSource::Explicit,
        });
    }
    out
}

/// The explicit bindings of `workflow`, the version stored at `workflow_node`.
pub fn explicit_bindings(
    view: &GraphView,
    workflow_node: &str,
    workflow: &CanonicalWorkflow,
) -> Vec<StepBinding> {
    resolve_bindings(view, workflow_node, workflow).bindings
}
