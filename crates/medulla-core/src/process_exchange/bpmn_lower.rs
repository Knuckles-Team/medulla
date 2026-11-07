//! The BPMN source's format mapping (MEDULLA-CQ-ORCH-R701.7, R701.11): the
//! parsed document viewed as a source graph, and the kind annotations of the
//! BPMN classes. Lower reads kinds only through these annotations.

use std::collections::HashMap;

use medulla_types::process_import::{
    AnnexEntry, GateKind, ImportError, KindAnnotation, LoweredFlow, ProcessSourceSystem,
    SourceGraph, SourceNode, StepKind, ANNEX_BPMN_PRESERVED_XML, PROCESS_IMPORT_SCHEMA_VERSION,
};

use super::{BpmnDocument, BpmnNode};

const fn annotate(class: &'static str, kind: StepKind, gate: Option<GateKind>) -> KindAnnotation {
    KindAnnotation {
        class,
        kind: Some(kind),
        gate,
    }
}

/// `:dagNodeKind` and `:canonicalGateKind` of each BPMN class. A complex
/// gateway has none and refuses `UnmappedElement`.
pub const BPMN_KIND_ANNOTATIONS: &[KindAnnotation] = &[
    annotate("userTask", StepKind::Human, None),
    annotate("manualTask", StepKind::Human, None),
    annotate("task", StepKind::Agent, None),
    annotate("serviceTask", StepKind::Agent, None),
    annotate("scriptTask", StepKind::Agent, None),
    annotate("sendTask", StepKind::Agent, None),
    annotate("intermediateThrowEvent", StepKind::Agent, None),
    annotate("businessRuleTask", StepKind::Decision, None),
    annotate("receiveTask", StepKind::Wait, None),
    annotate("intermediateCatchEvent", StepKind::Wait, None),
    annotate("eventBasedGateway", StepKind::Wait, Some(GateKind::Xor)),
    annotate("subProcess", StepKind::Call, None),
    annotate("callActivity", StepKind::Call, None),
    annotate("boundaryEvent", StepKind::Boundary, None),
    annotate("startEvent", StepKind::Start, None),
    annotate("endEvent", StepKind::End, None),
    annotate("exclusiveGateway", StepKind::Branch, Some(GateKind::Xor)),
    annotate("parallelGateway", StepKind::Fork, Some(GateKind::And)),
    annotate("inclusiveGateway", StepKind::Fork, Some(GateKind::Or)),
];

/// The trigger an event names: the first reference of its first event
/// definition, else the definition kind (`timer`, `signal`, ...).
fn trigger(node: &BpmnNode) -> Option<String> {
    if let Some(message) = node.attributes.get("messageRef") {
        return Some(message.clone());
    }
    let definition = node.event_definitions.first()?;
    definition
        .attributes
        .iter()
        .find(|(key, _)| key.ends_with("Ref"))
        .map(|(_, value)| value.clone())
        .or_else(|| {
            Some(
                definition
                    .kind
                    .trim_end_matches("EventDefinition")
                    .to_string(),
            )
        })
}

fn reference(node: &BpmnNode) -> Option<String> {
    match node.kind.as_str() {
        "businessRuleTask" => node.attributes.get("decisionRef").cloned(),
        "callActivity" => node.attributes.get("calledElement").cloned(),
        "receiveTask" | "intermediateCatchEvent" | "boundaryEvent" => trigger(node),
        _ => None,
    }
}

/// View the first process of a parsed BPMN document as a source graph. A
/// boundary event becomes a step reached from the activity it attaches to.
pub fn bpmn_source_graph(doc: &BpmnDocument) -> Result<SourceGraph, ImportError> {
    let process = doc
        .processes
        .first()
        .ok_or_else(|| ImportError::NotCanonical {
            reason: "the document has no process".into(),
        })?;
    let mut role: HashMap<&str, &str> = HashMap::new();
    for lane in &process.lanes {
        for node in &lane.flow_node_refs {
            role.insert(node.as_str(), lane.name.as_str());
        }
    }
    let nodes = process
        .nodes
        .iter()
        .map(|node| SourceNode {
            element_id: node.id.clone(),
            class: node.kind.clone(),
            label: node.name.clone(),
            role: role.get(node.id.as_str()).map(|r| r.to_string()),
            reference: reference(node),
        })
        .collect();
    let defaults: HashMap<&str, &str> = process
        .nodes
        .iter()
        .filter_map(|n| {
            n.attributes
                .get("default")
                .map(|f| (f.as_str(), n.id.as_str()))
        })
        .collect();
    let mut edges: Vec<LoweredFlow> = process
        .nodes
        .iter()
        .filter_map(|n| {
            n.attributes.get("attachedToRef").map(|host| LoweredFlow {
                source: host.clone(),
                target: n.id.clone(),
                condition: None,
                default: false,
            })
        })
        .collect();
    edges.extend(process.flows.iter().map(|flow| {
        LoweredFlow {
            source: flow.source_ref.clone(),
            target: flow.target_ref.clone(),
            condition: flow
                .condition
                .clone()
                .filter(|c| !c.trim().is_empty())
                .or_else(|| Some(flow.name.clone()).filter(|n| !n.trim().is_empty())),
            default: defaults.get(flow.id.as_str()) == Some(&flow.source_ref.as_str()),
        }
    }));
    let annex = process
        .preserved
        .iter()
        .chain(&doc.preserved)
        .map(|kept| AnnexEntry {
            schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
            after: None,
            before: None,
            construct: ANNEX_BPMN_PRESERVED_XML.to_string(),
            element_id: kept.owner.clone(),
            label: kept.xml.clone(),
            type_num: None,
        })
        .collect();
    Ok(SourceGraph {
        name: process.name.clone(),
        system: ProcessSourceSystem::Bpmn20,
        model_id: process.id.clone(),
        nodes,
        edges,
        annex,
    })
}
