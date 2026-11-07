//! Canonical workflow records (MEDULLA-CQ-ORCH-R701.1).
//!
//! One canonical workflow is the wire form of the steps and hand-offs of a
//! stored `WorkflowPattern` (MEDULLA-ORCH-VOCAB-R800.1). Every process source
//! imports to it and exports from it. The step kinds are closed to the values
//! of `:dagNodeKind` (MEDULLA-ORCH-VOCAB-R014.9); the source-only kind `state`
//! is not a step kind and never parses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The fixed `schema` of every canonical workflow.
pub const PROCESS_CANONICAL_SCHEMA: &str = "medulla.process.canonical/v1";

/// Current record version of the canonical workflow types.
pub const PROCESS_IMPORT_SCHEMA_VERSION: u16 = 1;

fn schema_version() -> u16 {
    PROCESS_IMPORT_SCHEMA_VERSION
}

/// The closed step kinds, in their canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Start,
    End,
    Human,
    Agent,
    Decision,
    Branch,
    Fork,
    Join,
    Wait,
    Call,
    Boundary,
}

/// The gate of a split, a join or an event-based wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    And,
    Or,
    Xor,
}

/// One canonical step. `id` is `s1` to `sn` in canonical order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalStep {
    pub id: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateKind>,
    pub label: String,
    /// The performer: a lane, a candidate group or an organisational element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The decision, the called process or the trigger the step names.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// One canonical flow between two step ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalFlow {
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub default: bool,
}

/// One step of a source process before canonical ids are assigned: the
/// output of a source's Lower stage (MEDULLA-CQ-ORCH-R701.2 rules 1, 2 and 6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoweredStep {
    pub element_id: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateKind>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// One flow between two lowered steps, named by source element id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoweredFlow {
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub default: bool,
}

/// One element of a source document viewed as a graph: the input of Lower
/// (MEDULLA-CQ-ORCH-R701.11). `class` is the source element class; Lower
/// reads its kind only through a [`KindAnnotation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceNode {
    pub element_id: String,
    pub class: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// The source document as a typed graph, with the annex its parse kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceGraph {
    pub name: String,
    pub system: ProcessSourceSystem,
    pub model_id: String,
    pub nodes: Vec<SourceNode>,
    pub edges: Vec<LoweredFlow>,
    #[serde(default)]
    pub annex: Vec<AnnexEntry>,
}

/// The step kind and gate a source class carries (`:dagNodeKind`,
/// `:canonicalGateKind`). `kind: None` is the source-only kind `state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindAnnotation {
    pub class: &'static str,
    pub kind: Option<StepKind>,
    pub gate: Option<GateKind>,
}

/// The output of Lower: steps and flows with no canonical id yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoweredProcess {
    pub name: String,
    pub system: ProcessSourceSystem,
    pub model_id: String,
    pub steps: Vec<LoweredStep>,
    pub flows: Vec<LoweredFlow>,
    #[serde(default)]
    pub annex: Vec<AnnexEntry>,
}

/// The output of Allocate: canonical ids `s1` to `sn` and sorted flows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocatedProcess {
    pub name: String,
    pub system: ProcessSourceSystem,
    pub model_id: String,
    pub steps: Vec<CanonicalStep>,
    pub flows: Vec<CanonicalFlow>,
    /// Step id to source element id.
    pub elements: BTreeMap<String, String>,
    #[serde(default)]
    pub annex: Vec<AnnexEntry>,
}

/// The system a canonical workflow was imported from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSourceSystem {
    Bpmn20,
    ArisEpc,
}

/// Where each step came from: step id to source element id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTrace {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub system: ProcessSourceSystem,
    pub model_id: String,
    #[serde(default)]
    pub elements: BTreeMap<String, String>,
}

/// One source-only construct kept for a same-source re-export.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnexEntry {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub construct: String,
    pub element_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_num: Option<u32>,
}

/// One construct a target format cannot hold, and how it was treated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessLoss {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub element: String,
    pub construct: String,
    pub target: ProcessSourceSystem,
    pub treatment: String,
}

/// The canonical workflow. Only `schema`, `steps` and `flows` enter `digest`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalWorkflow {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub schema: String,
    pub name: String,
    pub steps: Vec<CanonicalStep>,
    pub flows: Vec<CanonicalFlow>,
    pub digest: String,
    pub source: SourceTrace,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annex: Vec<AnnexEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub losses: Vec<ProcessLoss>,
}

/// Why a process document was not admitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportError {
    UnmappedElement { element_type: String },
    NotCanonical { reason: String },
    DigestMismatch { stated: String, computed: String },
    UnknownSource { system: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnmappedElement { element_type } => {
                write!(f, "process element type {element_type} has no step kind")
            }
            Self::NotCanonical { reason } => write!(f, "process is not canonical: {reason}"),
            Self::DigestMismatch { stated, computed } => {
                write!(
                    f,
                    "process digest {stated} differs from computed {computed}"
                )
            }
            Self::UnknownSource { system } => write!(f, "unknown process source {system}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Annex construct: BPMN diagram interchange or extension XML, restored on
/// a BPMN re-export (MEDULLA-CQ-ORCH-R702.8).
pub const ANNEX_BPMN_PRESERVED_XML: &str = "bpmn-preserved-xml";

/// Annex construct: an intermediate EPC state event.
pub const ANNEX_EPC_STATE_EVENT: &str = "epc-state-event";
/// Annex construct: an EPC event after a split that carries an arm condition.
pub const ANNEX_EPC_CONDITION_EVENT: &str = "epc-condition-event";
/// Annex construct: an EPC attachment other than performer, IT support or rule.
pub const ANNEX_EPC_ATTACHMENT: &str = "epc-attachment";

/// The canonical constructs EPC cannot express, and the one treatment the
/// EPC adapter applies to each (MEDULLA-CQ-ORCH-R701.8).
pub const EPC_EXPORT_TREATMENTS: &[(&str, &str)] = &[
    ("wait", "typed-trigger"),
    ("boundary", "boundary-event"),
    ("default-flow", "default-flow"),
    ("event-based-wait", "event-based-gateway"),
    ("decision", "decision-table"),
];

/// The treatment of an annex entry a target cannot hold.
pub const LOSS_ANNEX_NOT_HELD: &str = "annex-not-held";

impl ProcessSourceSystem {
    /// The annex constructs this source keeps for its own re-export.
    pub fn annex_constructs(self) -> &'static [&'static str] {
        match self {
            Self::ArisEpc => &[
                ANNEX_EPC_STATE_EVENT,
                ANNEX_EPC_CONDITION_EVENT,
                ANNEX_EPC_ATTACHMENT,
            ],
            Self::Bpmn20 => &[ANNEX_BPMN_PRESERVED_XML],
        }
    }

    /// The canonical constructs this format cannot express, with treatments.
    pub fn inexpressible(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::ArisEpc => EPC_EXPORT_TREATMENTS,
            Self::Bpmn20 => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workflow() -> serde_json::Value {
        json!({
            "schema": PROCESS_CANONICAL_SCHEMA,
            "name": "Invoice approval",
            "steps": [
                {"id": "s1", "kind": "start", "label": "Invoice received"},
                {"id": "s2", "kind": "branch", "gate": "xor", "label": "Amount?"},
                {"id": "s3", "kind": "human", "label": "Approve", "role": "Finance"},
                {"id": "s4", "kind": "decision", "label": "Check", "ref": "limits"},
                {"id": "s5", "kind": "end", "label": "Done"}
            ],
            "flows": [
                {"source": "s1", "target": "s2"},
                {"source": "s2", "target": "s3", "condition": "high"},
                {"source": "s2", "target": "s4", "default": true},
                {"source": "s3", "target": "s5"},
                {"source": "s4", "target": "s5"}
            ],
            "digest": "0".repeat(64),
            "source": {"system": "aris_epc", "model_id": "m1", "elements": {"s1": "e1"}},
            "annex": [{"construct": "epc-state-event", "element_id": "e9", "after": "s2"}],
            "losses": []
        })
    }

    // spec: MEDULLA-CQ-ORCH-R701.1
    #[test]
    fn canonical_workflow_round_trips() {
        let parsed: CanonicalWorkflow = serde_json::from_value(workflow()).unwrap();
        assert_eq!(parsed.steps[3].reference.as_deref(), Some("limits"));
        assert_eq!(parsed.steps[1].gate, Some(GateKind::Xor));
        assert_eq!(parsed.schema_version, PROCESS_IMPORT_SCHEMA_VERSION);
        let encoded = serde_json::to_value(&parsed).unwrap();
        assert_eq!(encoded["steps"][3]["ref"], "limits");
        let again: CanonicalWorkflow = serde_json::from_value(encoded).unwrap();
        assert_eq!(again, parsed);

        for (pointer, value) in [
            ("/steps/0/kind", json!("subprocess")),
            ("/steps/1/gate", json!("complex")),
            ("/steps/0/kind", json!("state")),
            ("/source/system", json!("yawl")),
        ] {
            let mut bad = workflow();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(
                serde_json::from_value::<CanonicalWorkflow>(bad).is_err(),
                "{pointer}"
            );
        }
        let mut extra = workflow();
        extra["steps"][0]["colour"] = json!("red");
        assert!(serde_json::from_value::<CanonicalWorkflow>(extra).is_err());
    }
}
