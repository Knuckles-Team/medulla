//! BPMN document to graph mapping (MEDULLA-CQ-ORCH-R702.3).
//!
//! One pure function turns a parsed BPMN document into the graph writes the served
//! `ProcessDocumentImport` handler commits as one batch. Each element becomes one node of its
//! `bpmn-v1.ttl` class with its `bpmnElementId`; the node id is `process:<model_iri>:<element id>`.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BpmnDocument, BpmnEventDefinition, BpmnLane, BpmnNode, BpmnProcess, BpmnSequenceFlow,
    PreservedXml, BPMN_NODE_KINDS,
};
use crate::architecture_exchange::graph::ExchangeMutation;
use medulla_types::process_exchange::ProcessExchangeError;

/// Property prefix of a kept source attribute of a flow node or an event definition.
const ATTRIBUTE_PREFIX: &str = "bpmnAttribute:";
/// Property prefix of a namespace the root element declares.
const NAMESPACE_PREFIX: &str = "bpmnNamespace:";
/// Property of the definitions node that keeps the collaborations and their participants, as a
/// JSON array in document order, so an export restores them (MEDULLA-CQ-ORCH-R702.9).
const COLLABORATIONS_PROPERTY: &str = "bpmnCollaborations";

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

    let mut push_node = |id: &str, class: &str, name: &str, extra: &[(String, String)]| {
        let mut properties = named(name);
        for (key, value) in extra {
            properties.insert(key.clone(), value.clone());
        }
        nodes.push((id.to_owned(), node(class, iri(id), id, properties)));
        known.insert(id.to_owned());
    };
    let mut push_edge = |from: &str, to: &str, predicate: &str| {
        edges.push((
            format!("{from}\u{0}{predicate}\u{0}{to}"),
            link(&iri(from), iri(to), predicate),
        ));
    };

    let mut namespaces: Vec<(String, String)> = doc
        .namespaces
        .iter()
        .map(|(prefix, iri)| (format!("{NAMESPACE_PREFIX}{prefix}"), iri.clone()))
        .collect();
    if !doc.collaborations.is_empty() {
        if let Ok(text) = serde_json::to_string(&doc.collaborations) {
            namespaces.push((COLLABORATIONS_PROPERTY.to_owned(), text));
        }
    }
    push_node(&doc.id, "BpmnDefinitions", "", &namespaces);
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
                let mut kept = definition_properties(definition);
                kept.push(("bpmnKind".to_owned(), definition.kind.clone()));
                push_node(
                    &definition_id,
                    &definition_class(&definition.kind),
                    "",
                    &kept,
                );
                push_edge(&element.id, &definition_id, "hasEventDefinition");
            }
        }
        for flow in &process.flows {
            let mut extra = vec![(
                "isDefaultFlow".to_owned(),
                defaults.contains(flow.id.as_str()).to_string(),
            )];
            if let Some(condition) = &flow.condition {
                extra.push(("flowCondition".to_owned(), condition.clone()));
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
        let owner = if known.contains(owner) {
            owner
        } else {
            doc.id.as_str()
        };
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

fn event_properties(element: &BpmnNode) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = event_position(&element.kind)
        .map(|position| vec![("eventPosition".to_owned(), position.to_owned())])
        .unwrap_or_default();
    out.extend(
        element
            .attributes
            .iter()
            .map(|(name, value)| (format!("{ATTRIBUTE_PREFIX}{name}"), value.clone())),
    );
    out
}

fn definition_properties(definition: &BpmnEventDefinition) -> Vec<(String, String)> {
    definition
        .attributes
        .iter()
        .map(|(name, value)| (format!("{ATTRIBUTE_PREFIX}{name}"), value.clone()))
        .collect()
}

/// The element id of a node id of the model `model_iri`, or `None` for a foreign node.
fn element_of<'a>(model_iri: &str, node_id: &'a str) -> Option<&'a str> {
    node_id
        .strip_prefix("process:")
        .and_then(|rest| rest.strip_prefix(model_iri))
        .and_then(|rest| rest.strip_prefix(':'))
}

/// The BPMN element local name of a class label: `BpmnUserTask` is `userTask`.
fn kind_of_class(label: &str) -> Option<String> {
    let rest = label.strip_prefix("Bpmn")?;
    let mut chars = rest.chars();
    let head = chars.next()?.to_ascii_lowercase();
    Some(format!("{head}{}", chars.as_str()))
}

fn attributes_of(properties: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    properties
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(ATTRIBUTE_PREFIX)
                .map(|name| (name.to_owned(), value.clone()))
        })
        .collect()
}

fn not_deployable(reason: impl Into<String>) -> ProcessExchangeError {
    ProcessExchangeError::NotDeployable {
        reason: reason.into(),
    }
}

/// The inverse of [`bpmn_mutations`]: rebuild the BPMN document of the model `model_iri` from
/// the node and edge writes that stored it (MEDULLA-CQ-ORCH-R702.10). A model without one
/// `BpmnDefinitions` node refuses `NotDeployable`. Elements come back in element id order, a
/// lane's members in id order, an element's event definitions in their stored order. The
/// collaborations and their participants come back from the definitions node's
/// `bpmnCollaborations` property.
pub fn bpmn_document(
    mutations: &[ExchangeMutation],
    model_iri: &str,
) -> Result<BpmnDocument, ProcessExchangeError> {
    let mut nodes: BTreeMap<&str, (&str, &BTreeMap<String, String>)> = BTreeMap::new();
    let mut edges: Vec<(&str, &str, &str)> = Vec::new();
    for mutation in mutations {
        match mutation {
            ExchangeMutation::UpsertNode {
                id,
                label,
                properties,
            } => {
                if let Some(element) = element_of(model_iri, id) {
                    nodes.insert(element, (label.as_str(), properties));
                }
            }
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => {
                if let (Some(from), Some(to)) =
                    (element_of(model_iri, source), element_of(model_iri, target))
                {
                    edges.push((from, predicate.as_str(), to));
                }
            }
        }
    }
    let targets = |from: &str, predicate: &str| -> Vec<&str> {
        let mut out: Vec<&str> = edges
            .iter()
            .filter(|(source, p, _)| *source == from && *p == predicate)
            .map(|(_, _, target)| *target)
            .collect();
        out.sort_unstable();
        out
    };
    let sources = |to: &str, predicate: &str| -> Vec<&str> {
        let mut out: Vec<&str> = edges
            .iter()
            .filter(|(_, p, target)| *target == to && *p == predicate)
            .map(|(source, _, _)| *source)
            .collect();
        out.sort_unstable();
        out
    };
    let name_of =
        |properties: &BTreeMap<String, String>| properties.get("name").cloned().unwrap_or_default();
    let kept = |owner: &str, properties: &BTreeMap<String, String>| {
        properties.get("bpmnPreservedXml").map(|xml| PreservedXml {
            owner: owner.to_owned(),
            xml: xml.clone(),
        })
    };

    let mut definitions = nodes
        .iter()
        .filter(|(_, (label, _))| *label == "BpmnDefinitions");
    let (Some((definitions_id, (_, definitions_properties))), None) =
        (definitions.next(), definitions.next())
    else {
        return Err(not_deployable(
            "the model has no single BpmnDefinitions node",
        ));
    };
    let mut doc = BpmnDocument {
        id: (*definitions_id).to_owned(),
        namespaces: definitions_properties
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(NAMESPACE_PREFIX)
                    .map(|prefix| (prefix.to_owned(), value.clone()))
            })
            .collect(),
        collaborations: definitions_properties
            .get(COLLABORATIONS_PROPERTY)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default(),
        ..BpmnDocument::default()
    };
    doc.preserved.extend(kept(&doc.id, definitions_properties));

    for (process_id, (label, properties)) in &nodes {
        if *label != "BpmnProcess" {
            continue;
        }
        let mut process = BpmnProcess {
            id: (*process_id).to_owned(),
            name: name_of(properties),
            ..BpmnProcess::default()
        };
        process.preserved.extend(kept(process_id, properties));
        for element_id in targets(process_id, "hasProcessElement") {
            let Some((class, element)) = nodes.get(element_id) else {
                continue;
            };
            process.preserved.extend(kept(element_id, element));
            match *class {
                "BpmnLane" => process.lanes.push(BpmnLane {
                    id: element_id.to_owned(),
                    name: name_of(element),
                    flow_node_refs: sources(element_id, "inLane")
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }),
                "BpmnSequenceFlow" => {
                    let endpoint = |predicate: &str| {
                        targets(element_id, predicate)
                            .first()
                            .map(|id| (*id).to_owned())
                            .ok_or_else(|| {
                                not_deployable(format!(
                                    "sequence flow {element_id} has no {predicate} edge"
                                ))
                            })
                    };
                    process.flows.push(BpmnSequenceFlow {
                        id: element_id.to_owned(),
                        name: name_of(element),
                        source_ref: endpoint("flowSource")?,
                        target_ref: endpoint("flowTarget")?,
                        condition: element.get("flowCondition").cloned(),
                    });
                }
                other => {
                    let kind = kind_of_class(other)
                        .filter(|kind| BPMN_NODE_KINDS.contains(&kind.as_str()))
                        .ok_or_else(|| {
                            not_deployable(format!(
                                "element {element_id} has the class {other}, which is not a flow node"
                            ))
                        })?;
                    let mut definitions: Vec<(usize, BpmnEventDefinition)> = Vec::new();
                    for definition_id in targets(element_id, "hasEventDefinition") {
                        let Some((definition_class, definition)) = nodes.get(definition_id) else {
                            continue;
                        };
                        let index = definition_id
                            .rsplit_once('#')
                            .and_then(|(_, index)| index.parse::<usize>().ok())
                            .unwrap_or(usize::MAX);
                        let kind = definition
                            .get("bpmnKind")
                            .cloned()
                            .or_else(|| kind_of_class(definition_class))
                            .unwrap_or_default();
                        definitions.push((
                            index,
                            BpmnEventDefinition {
                                kind,
                                attributes: attributes_of(definition),
                            },
                        ));
                    }
                    definitions.sort_by_key(|(index, _)| *index);
                    process.nodes.push(BpmnNode {
                        id: element_id.to_owned(),
                        kind,
                        name: name_of(element),
                        attributes: attributes_of(element),
                        event_definitions: definitions
                            .into_iter()
                            .map(|(_, definition)| definition)
                            .collect(),
                    });
                }
            }
        }
        doc.processes.push(process);
    }
    Ok(doc)
}
