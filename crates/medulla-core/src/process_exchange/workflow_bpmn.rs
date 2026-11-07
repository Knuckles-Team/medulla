//! Stored workflow to deployable BPMN (MEDULLA-CQ-ORCH-R702.8).

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::process_import::{
    CanonicalStep, CanonicalWorkflow, GateKind, StepKind, ANNEX_BPMN_PRESERVED_XML,
};

use super::{
    BpmnDocument, BpmnEventDefinition, BpmnLane, BpmnNode, BpmnProcess, BpmnSequenceFlow,
    PreservedXml, ProcessExchangeError,
};

fn gateway(gate: Option<GateKind>) -> &'static str {
    match gate.unwrap_or(GateKind::Xor) {
        GateKind::Xor => "exclusiveGateway",
        GateKind::And => "parallelGateway",
        GateKind::Or => "inclusiveGateway",
    }
}

/// The BPMN element local name of one canonical step.
fn element_kind(step: &CanonicalStep) -> &'static str {
    match step.kind {
        StepKind::Start => "startEvent",
        StepKind::End => "endEvent",
        StepKind::Human => "userTask",
        StepKind::Agent => "serviceTask",
        StepKind::Decision => "businessRuleTask",
        StepKind::Wait if step.gate.is_some() => "eventBasedGateway",
        StepKind::Wait => "intermediateCatchEvent",
        StepKind::Call => "callActivity",
        StepKind::Boundary => "boundaryEvent",
        StepKind::Branch | StepKind::Fork | StepKind::Join => gateway(step.gate),
    }
}

/// A valid XML name for a canonical id (`s1` stays `s1`).
fn xml_id(id: &str) -> String {
    let mut out: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        out.insert(0, '_');
    }
    out
}

fn node(step: &CanonicalStep) -> BpmnNode {
    let mut attributes = BTreeMap::new();
    let mut event_definitions = Vec::new();
    match (step.kind, &step.reference) {
        (StepKind::Decision, Some(reference)) => {
            attributes.insert("decisionRef".to_string(), reference.clone());
        }
        (StepKind::Call, Some(reference)) => {
            attributes.insert("calledElement".to_string(), reference.clone());
        }
        (StepKind::Wait | StepKind::Boundary, Some(reference)) if step.gate.is_none() => {
            event_definitions.push(BpmnEventDefinition {
                kind: "messageEventDefinition".to_string(),
                attributes: BTreeMap::from([("messageRef".to_string(), reference.clone())]),
            });
        }
        _ => {}
    }
    BpmnNode {
        id: xml_id(&step.id),
        kind: element_kind(step).to_string(),
        name: step.label.clone(),
        attributes,
        event_definitions,
    }
}

/// Export a stored canonical workflow as one deployable BPMN process. A
/// boundary step attaches to the activity of its incoming flow. One start and
/// one end event are added when the workflow has none.
pub fn workflow_to_bpmn(
    workflow: &CanonicalWorkflow,
) -> Result<BpmnDocument, ProcessExchangeError> {
    if workflow.steps.is_empty() {
        return Err(ProcessExchangeError::NotDeployable {
            reason: "the workflow has no step".into(),
        });
    }
    let kind_of: BTreeMap<&str, &CanonicalStep> =
        workflow.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut process = BpmnProcess {
        id: xml_id(&format!(
            "process_{}",
            &workflow.digest[..workflow.digest.len().min(12)]
        )),
        name: workflow.name.clone(),
        ..BpmnProcess::default()
    };
    let mut nodes: Vec<BpmnNode> = workflow.steps.iter().map(node).collect();
    let mut flows = Vec::new();
    for (n, flow) in workflow.flows.iter().enumerate() {
        let id = format!("f{}", n + 1);
        if kind_of.get(flow.target.as_str()).map(|s| s.kind) == Some(StepKind::Boundary) {
            if let Some(boundary) = nodes.iter_mut().find(|b| b.id == xml_id(&flow.target)) {
                if !boundary.attributes.contains_key("attachedToRef") {
                    boundary
                        .attributes
                        .insert("attachedToRef".to_string(), xml_id(&flow.source));
                    continue;
                }
            }
        }
        if flow.default {
            if let Some(source) = nodes.iter_mut().find(|s| s.id == xml_id(&flow.source)) {
                source.attributes.insert("default".to_string(), id.clone());
            }
        }
        flows.push(BpmnSequenceFlow {
            id,
            name: String::new(),
            source_ref: xml_id(&flow.source),
            target_ref: xml_id(&flow.target),
            condition: flow.condition.clone(),
        });
    }
    let has = |kind: StepKind| workflow.steps.iter().any(|s| s.kind == kind);
    if !has(StepKind::Start) {
        let targets: BTreeSet<&str> = workflow.flows.iter().map(|f| f.target.as_str()).collect();
        nodes.push(BpmnNode {
            id: "start".into(),
            kind: "startEvent".into(),
            ..BpmnNode::default()
        });
        for step in workflow
            .steps
            .iter()
            .filter(|s| !targets.contains(s.id.as_str()))
        {
            flows.push(BpmnSequenceFlow {
                id: format!("f{}", flows.len() + 1),
                source_ref: "start".into(),
                target_ref: xml_id(&step.id),
                ..BpmnSequenceFlow::default()
            });
        }
    }
    if !has(StepKind::End) {
        let sources: BTreeSet<&str> = workflow.flows.iter().map(|f| f.source.as_str()).collect();
        nodes.push(BpmnNode {
            id: "end".into(),
            kind: "endEvent".into(),
            ..BpmnNode::default()
        });
        for step in workflow
            .steps
            .iter()
            .filter(|s| !sources.contains(s.id.as_str()))
        {
            flows.push(BpmnSequenceFlow {
                id: format!("f{}", flows.len() + 1),
                source_ref: xml_id(&step.id),
                target_ref: "end".into(),
                ..BpmnSequenceFlow::default()
            });
        }
    }
    let mut lanes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for step in &workflow.steps {
        if let Some(role) = &step.role {
            lanes
                .entry(role.as_str())
                .or_default()
                .push(xml_id(&step.id));
        }
    }
    process.lanes = lanes
        .into_iter()
        .enumerate()
        .map(|(n, (role, refs))| BpmnLane {
            id: format!("lane{}", n + 1),
            name: role.to_string(),
            flow_node_refs: refs,
        })
        .collect();
    process.nodes = nodes;
    process.flows = flows;
    process.preserved = workflow
        .annex
        .iter()
        .filter(|entry| entry.construct == ANNEX_BPMN_PRESERVED_XML)
        .map(|entry| PreservedXml {
            owner: entry.element_id.clone(),
            xml: entry.label.clone(),
        })
        .collect();
    Ok(BpmnDocument {
        id: xml_id(&format!(
            "definitions_{}",
            &workflow.digest[..workflow.digest.len().min(12)]
        )),
        processes: vec![process],
        collaborations: Vec::new(),
        preserved: Vec::new(),
        namespaces: BTreeMap::new(),
    })
}
