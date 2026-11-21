//! BPMN document to graph mapping (MEDULLA-CQ-ORCH-R702.3).
//!
//! One pure function turns a parsed BPMN document into the graph writes the served
//! `ProcessDocumentImport` handler commits as one batch. Each element becomes one node of its
//! `bpmn-v1.ttl` class with its `bpmnElementId`; the node id is `process:<model_iri>:<element id>`.

use std::collections::{BTreeMap, BTreeSet};

use super::{BpmnDocument, BpmnNode};
use crate::architecture_exchange::graph::ExchangeMutation;

/// The node id of one element of the model `model_iri`.
pub fn element_iri(model_iri: &str, element_id: &str) -> String {
    format!("process:{model_iri}:{element_id}")
}

/// The `bpmn-v1.ttl` class of a BPMN element local name: `userTask` is `BpmnUserTask`.
fn class_of(kind: &str) -> String {
    let mut chars = kind.chars();
    let head = chars.next().map(|c| c.to_ascii_uppercase());
    format!("Bpmn{}{}", head.unwrap_or_default(), chars.as_str())
}

/// The class of an event definition; a kind `bpmn-v1.ttl` has no class for is a plain
/// `BpmnEventDefinition`.
fn definition_class(kind: &str) -> String {
    const KINDS: [&str; 8] = [
        "timerEventDefinition",
        "messageEventDefinition",
        "signalEventDefinition",
        "errorEventDefinition",
        "escalationEventDefinition",
        "conditionalEventDefinition",
        "compensateEventDefinition",
        "terminateEventDefinition",
    ];
    if KINDS.contains(&kind) {
        class_of(kind)
    } else {
        "BpmnEventDefinition".to_owned()
    }
}

/// Where an event node sits in the flow, as `:eventPosition` states it.
fn event_position(kind: &str) -> Option<&'static str> {
    match kind {
        "startEvent" => Some("start"),
        "endEvent" => Some("end"),
        "intermediateCatchEvent" | "intermediateThrowEvent" => Some("intermediate"),
        "boundaryEvent" => Some("boundary"),
        _ => None,
    }
}

fn node(
    label: &str,
    id: String,
    element_id: &str,
    mut properties: BTreeMap<String, String>,
) -> ExchangeMutation {
    properties.insert("type".to_owned(), label.to_owned());
    properties.insert("bpmnElementId".to_owned(), element_id.to_owned());
    ExchangeMutation::UpsertNode {
        id,
        label: label.to_owned(),
        properties,
    }
}

fn link(source: &str, target: String, predicate: &str) -> ExchangeMutation {
    ExchangeMutation::Link {
        source: source.to_owned(),
        target,
        predicate: predicate.to_owned(),
    }
}

fn named(name: &str) -> BTreeMap<String, String> {
    let mut properties = BTreeMap::new();
    if !name.is_empty() {
        properties.insert("name".to_owned(), name.to_owned());
    }
    properties
}

/// Map a BPMN document onto graph writes. The document is one `BpmnDefinitions` node and each
/// process one `BpmnProcess` node, linked by `hasProcessElement` to every lane, flow node and
/// flow it holds. A flow node is a node of its `Bpmn` class; a sequence flow is a
/// `BpmnSequenceFlow` node with `flowSource` and `flowTarget` edges, its `flowCondition`, and
/// `isDefaultFlow` when its source gateway names it as the default. Lane membership is an
/// `inLane` edge. An event carries its `eventPosition` and one event definition node per
/// definition. A kept fragment is `bpmnPreservedXml` on its owner node. The list is ordered by
/// element id (nodes first, then edges), so equal input gives equal bytes. Nothing is written
/// here.
pub fn bpmn_mutations(doc: &BpmnDocument, model_iri: &str) -> Vec<ExchangeMutation> {
    let iri = |id: &str| element_iri(model_iri, id);
    let mut nodes: Vec<(String, ExchangeMutation)> = Vec::new();
    let mut edges: Vec<(String, ExchangeMutation)> = Vec::new();
    let mut known: BTreeSet<String> = BTreeSet::new();

    // Kept fragments, joined in document order per owner.
    let mut preserved: BTreeMap<&str, String> = BTreeMap::new();
    for fragment in doc
        .preserved
        .iter()
        .chain(doc.processes.iter().flat_map(|p| p.preserved.iter()))
    {
        preserved
            .entry(fragment.owner.as_str())
            .or_default()
            .push_str(&fragment.xml);
    }

    let mut push_node = |id: &str, class: &str, name: &str, extra: &[(&str, String)]| {
        let mut properties = named(name);
        for (key, value) in extra {
            properties.insert((*key).to_owned(), value.clone());
        }
        nodes.push((
            id.to_owned(),
            node(class, iri(id), id, properties),
        ));
        known.insert(id.to_owned());
    };
    let mut push_edge = |from: &str, to: &str, predicate: &str| {
        edges.push((
            format!("{from}\u{0}{predicate}\u{0}{to}"),
            link(&iri(from), iri(to), predicate),
        ));
    };

    push_node(&doc.id, "BpmnDefinitions", "", &[]);
    for process in &doc.processes {
        push_node(&process.id, "BpmnProcess", &process.name, &[]);
        push_edge(&doc.id, &process.id, "hasProcessElement");
        let defaults: BTreeSet<&str> = process
            .nodes
            .iter()
            .filter_map(|n| n.attributes.get("default").map(String::as_str))
            .collect();
        for lane in &process.lanes {
            push_node(&lane.id, "BpmnLane", &lane.name, &[]);
            push_edge(&process.id, &lane.id, "hasProcessElement");
            for member in &lane.flow_node_refs {
                push_edge(member, &lane.id, "inLane");
            }
        }
        for element in &process.nodes {
            push_node(
                &element.id,
                &class_of(&element.kind),
                &element.name,
                &event_properties(element),
            );
            push_edge(&process.id, &element.id, "hasProcessElement");
            for (index, definition) in element.event_definitions.iter().enumerate() {
                let definition_id = format!("{}#{index}", element.id);
                push_node(&definition_id, &definition_class(&definition.kind), "", &[]);
                push_edge(&element.id, &definition_id, "hasEventDefinition");
            }
        }
        for flow in &process.flows {
            let mut extra = vec![("isDefaultFlow", defaults.contains(flow.id.as_str()).to_string())];
            if let Some(condition) = &flow.condition {
                extra.push(("flowCondition", condition.clone()));
            }
            push_node(&flow.id, "BpmnSequenceFlow", &flow.name, &extra);
            push_edge(&process.id, &flow.id, "hasProcessElement");
            push_edge(&flow.id, &flow.source_ref, "flowSource");
            push_edge(&flow.id, &flow.target_ref, "flowTarget");
        }
    }
    drop(push_node);
    drop(push_edge);

    // A fragment whose owner is not an element of the document stays on the document node.
    let mut kept: BTreeMap<String, String> = BTreeMap::new();
    for (owner, xml) in preserved {
        let owner = if known.contains(owner) { owner } else { doc.id.as_str() };
        kept.entry(owner.to_owned()).or_default().push_str(&xml);
    }
    for (id, mutation) in &mut nodes {
        if let (Some(xml), ExchangeMutation::UpsertNode { properties, .. }) =
            (kept.get(id.as_str()), mutation)
        {
            properties.insert("bpmnPreservedXml".to_owned(), xml.clone());
        }
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    edges.sort_by(|a, b| a.0.cmp(&b.0));
    nodes
        .into_iter()
        .chain(edges)
        .map(|(_, mutation)| mutation)
        .collect()
}

fn event_properties(element: &BpmnNode) -> Vec<(&'static str, String)> {
    event_position(&element.kind)
        .map(|position| vec![("eventPosition", position.to_owned())])
        .unwrap_or_default()
}
