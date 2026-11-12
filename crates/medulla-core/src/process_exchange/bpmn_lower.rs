//! The BPMN source's format mapping (MEDULLA-CQ-ORCH-R701.7, R701.11): the
//! parsed document viewed as a source graph, and the kind annotations of the
//! BPMN classes, read from the ontology (`bpmn-v1.ttl`). Lower reads kinds
//! only through these annotations; no class is mapped in code.

use std::collections::HashMap;
use std::sync::OnceLock;

use medulla_types::process_import::{
    AnnexEntry, GateKind, ImportError, KindAnnotation, LoweredFlow, ProcessSourceSystem,
    SourceGraph, SourceNode, StepKind, ANNEX_BPMN_PRESERVED_XML, PROCESS_IMPORT_SCHEMA_VERSION,
};

use super::{BpmnDocument, BpmnNode};

/// The core namespace of the BPMN classes and the two annotations.
const KG: &str = medulla_types::ontology_iri!("core#");

/// The BPMN class vocabulary (MEDULLA-ORCH-VOCAB-R014.2, R014.9).
const BPMN_VOCABULARY: &str = include_str!("../../ontology/bpmn-v1.ttl");

/// The BPMN element local name of a `:Bpmn<Name>` class (`:BpmnUserTask`
/// is `userTask`).
fn element_name(class_iri: &str) -> Option<String> {
    let name = class_iri.strip_prefix(KG)?.strip_prefix("Bpmn")?;
    let mut chars = name.chars();
    let first = chars.next()?;
    Some(first.to_ascii_lowercase().to_string() + chars.as_str())
}

fn closed_value<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ImportError> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|_| {
        ImportError::NotCanonical {
            reason: format!("bpmn-v1.ttl carries the unknown kind value {value}"),
        }
    })
}

fn read_annotations() -> Result<Vec<KindAnnotation>, ImportError> {
    let mut kind: HashMap<String, String> = HashMap::new();
    let mut gate: HashMap<String, String> = HashMap::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut classes: Vec<String> = Vec::new();
    for triple in oxttl::TurtleParser::new().for_reader(BPMN_VOCABULARY.as_bytes()) {
        let triple = triple.map_err(|error| ImportError::NotCanonical {
            reason: format!("bpmn-v1.ttl: {error}"),
        })?;
        let oxrdf::NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            continue;
        };
        let subject = subject.as_str().to_string();
        let predicate = triple.predicate.as_str();
        match (&triple.object, predicate) {
            (oxrdf::Term::Literal(value), p) if p == format!("{KG}dagNodeKind") => {
                kind.insert(subject, value.value().to_string());
            }
            (oxrdf::Term::Literal(value), p) if p == format!("{KG}canonicalGateKind") => {
                gate.insert(subject, value.value().to_string());
            }
            (oxrdf::Term::NamedNode(object), "http://www.w3.org/2000/01/rdf-schema#subClassOf") => {
                parent.insert(subject, object.as_str().to_string());
            }
            (oxrdf::Term::NamedNode(object), "http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
                if object.as_str() == "http://www.w3.org/2002/07/owl#Class" =>
            {
                classes.push(subject);
            }
            _ => {}
        }
    }
    let nearest = |map: &HashMap<String, String>, class: &str| {
        let mut current = Some(class.to_string());
        let mut hops = 0;
        while let Some(iri) = current {
            if let Some(value) = map.get(&iri) {
                return Some(value.clone());
            }
            hops += 1;
            current = parent.get(&iri).cloned().filter(|_| hops < 32);
        }
        None
    };
    let mut annotations = Vec::new();
    for class in &classes {
        let (Some(element), Some(value)) = (element_name(class), nearest(&kind, class)) else {
            continue;
        };
        annotations.push(KindAnnotation {
            class: Box::leak(element.into_boxed_str()),
            kind: if value == "state" {
                None
            } else {
                Some(closed_value::<StepKind>(&value)?)
            },
            gate: nearest(&gate, class)
                .map(|g| closed_value::<GateKind>(&g))
                .transpose()?,
        });
    }
    Ok(annotations)
}

/// The `:dagNodeKind` and `:canonicalGateKind` of every BPMN class, read
/// once from `bpmn-v1.ttl` along each class's `rdfs:subClassOf` closure. A
/// class without a kind (a complex gateway) is absent and so refuses
/// `UnmappedElement`.
pub fn bpmn_kind_annotations() -> Result<&'static [KindAnnotation], ImportError> {
    static ANNOTATIONS: OnceLock<Result<Vec<KindAnnotation>, ImportError>> = OnceLock::new();
    ANNOTATIONS
        .get_or_init(read_annotations)
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

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
