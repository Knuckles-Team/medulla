//! Native BPMN 2.0 documents (MEDULLA-CQ-ORCH-R702).
//!
//! [`BpmnDocument`] is the tree the reader returns (`bpmn_read::parse_bpmn`,
//! R702.2) and the export builds from a stored workflow
//! (`workflow_bpmn::workflow_to_bpmn`, R702.8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod bpmn_graph;
pub mod bpmn_lower;
pub mod bpmn_read;
pub mod bpmn_write;
pub mod dmn_graph;
pub mod dmn_read;
pub mod dmn_write;
pub mod workflow_bpmn;
pub mod xml_tree;

pub use medulla_types::process_exchange::ProcessExchangeError;

/// `text` with the version scope of its model removed: a BPMN node id `process:<scope>:<id>`
/// becomes `process::<id>` and a DMN node id `<scope>#<id>` becomes `#<id>`; other text is
/// unchanged.
fn unscoped(text: &str, scope: &str) -> String {
    if let Some(rest) = text
        .strip_prefix("process:")
        .and_then(|rest| rest.strip_prefix(scope))
        .filter(|rest| rest.starts_with(':'))
    {
        return format!("process:{rest}");
    }
    match text.strip_prefix(scope) {
        Some(rest) if rest.starts_with('#') => rest.to_owned(),
        _ => text.to_owned(),
    }
}

/// The canonical model digest of one stored process or decision model (MEDULLA-CQ-ORCH-R702.13):
/// SHA-256, as 64 lowercase hex characters, of its node and edge writes with nodes ordered by
/// id and edges by (source, predicate, target), so the digest is the same whether the writes
/// come from the import mapping or are read back from the graph. `scope` is the version scope
/// the writes were mapped under (`<model_iri>:v<n>`, MEDULLA-CQ-ORCH-R702.9); it is removed from
/// every node id and id-valued property first, so two imports of one document into different
/// models or versions have the same digest (MEDULLA-CQ-ORCH-R702.14, R702.15).
pub fn canonical_model_digest(
    mutations: &[crate::architecture_exchange::graph::ExchangeMutation],
    scope: &str,
) -> String {
    use crate::architecture_exchange::graph::ExchangeMutation;
    let mut nodes: Vec<(String, &str, BTreeMap<&str, String>)> = Vec::new();
    let mut edges: Vec<(String, &str, String)> = Vec::new();
    for mutation in mutations {
        match mutation {
            ExchangeMutation::UpsertNode {
                id,
                label,
                properties,
            } => nodes.push((
                unscoped(id, scope),
                label.as_str(),
                properties
                    .iter()
                    .map(|(key, value)| (key.as_str(), unscoped(value, scope)))
                    .collect(),
            )),
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => edges.push((
                unscoped(source, scope),
                predicate.as_str(),
                unscoped(target, scope),
            )),
        }
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    edges.sort_unstable();
    edges.dedup();
    let value = serde_json::json!({"nodes": nodes, "edges": edges});
    let text = medulla_query_types::solve_scalar::digest_text("process-model", &value);
    text.strip_prefix(medulla_query_types::solve_scalar::DIGEST_TEXT_PREFIX)
        .unwrap_or(&text)
        .to_owned()
}

/// The BPMN 2.0 model namespace; the root element must be in it.
pub const BPMN_MODEL_NS: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";

/// A parsed BPMN 2.0 document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnDocument {
    pub id: String,
    pub processes: Vec<BpmnProcess>,
    pub collaborations: Vec<BpmnCollaboration>,
    /// Elements outside every process (diagram interchange, extensions).
    pub preserved: Vec<PreservedXml>,
    /// The prefixed namespaces the root declares (prefix to IRI), so that a
    /// written document keeps its preserved fragments well formed.
    pub namespaces: BTreeMap<String, String>,
}

/// One `process` with its lanes, flow nodes and sequence flows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnProcess {
    pub id: String,
    pub name: String,
    pub lanes: Vec<BpmnLane>,
    pub nodes: Vec<BpmnNode>,
    pub flows: Vec<BpmnSequenceFlow>,
    pub preserved: Vec<PreservedXml>,
}

/// One lane and the flow nodes it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnLane {
    pub id: String,
    pub name: String,
    pub flow_node_refs: Vec<String>,
}

/// One task, gateway or event. `kind` is the BPMN element local name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnNode {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// Attributes other than `id` and `name`, e.g. `default`, `calledElement`.
    pub attributes: BTreeMap<String, String>,
    pub event_definitions: Vec<BpmnEventDefinition>,
}

/// One event definition of an event, e.g. `timerEventDefinition`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnEventDefinition {
    pub kind: String,
    pub attributes: BTreeMap<String, String>,
}

/// One sequence flow with its condition expression.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnSequenceFlow {
    pub id: String,
    pub name: String,
    pub source_ref: String,
    pub target_ref: String,
    pub condition: Option<String>,
}

/// One collaboration and its participants (participant id, process ref).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpmnCollaboration {
    pub id: String,
    pub participants: Vec<(String, String)>,
}

/// An element the reader does not model, kept as XML with its owner id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreservedXml {
    pub owner: String,
    pub xml: String,
}

/// The BPMN flow node kinds the reader models.
pub const BPMN_NODE_KINDS: &[&str] = &[
    "task",
    "userTask",
    "manualTask",
    "serviceTask",
    "scriptTask",
    "sendTask",
    "receiveTask",
    "businessRuleTask",
    "subProcess",
    "callActivity",
    "exclusiveGateway",
    "parallelGateway",
    "inclusiveGateway",
    "eventBasedGateway",
    "complexGateway",
    "startEvent",
    "endEvent",
    "intermediateCatchEvent",
    "intermediateThrowEvent",
    "boundaryEvent",
];
