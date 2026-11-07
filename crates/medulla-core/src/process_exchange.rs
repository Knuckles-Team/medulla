//! Native BPMN 2.0 documents (MEDULLA-CQ-ORCH-R702).
//!
//! [`BpmnDocument`] is the tree the reader returns (`bpmn_read::parse_bpmn`,
//! R702.2) and the export builds from a stored workflow
//! (`workflow_bpmn::workflow_to_bpmn`, R702.8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod bpmn_lower;
pub mod bpmn_read;
pub mod workflow_bpmn;

pub use medulla_types::process_exchange::ProcessExchangeError;

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
