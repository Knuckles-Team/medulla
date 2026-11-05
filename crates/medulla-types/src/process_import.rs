//! Imported process model and gate classification (MEDULLA-CQ-ORCH-R701.1).
//!
//! Record types for the served `ProcessModelImport`. Pure data: the importer
//! (`import_epc`, R701.2) lives above this crate.

use serde::{Deserialize, Serialize};

/// Format identity of the imported-process record.
pub const IMPORTED_PROCESS_SCHEMA_VERSION: u16 = 1;

/// How a gate combines its branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GateKind {
    And,
    Or,
    Xor,
}

/// One classified gate of the imported model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImportedGate {
    pub element_id: String,
    pub kind: GateKind,
}

/// A directed flow between two element ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImportedFlow {
    pub from: String,
    pub to: String,
}

/// A process model imported from a source system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ImportedProcess {
    pub schema_version: u16,
    pub model_id: String,
    pub source_system: String,
    pub steps: Vec<String>,
    pub gates: Vec<ImportedGate>,
    pub flows: Vec<ImportedFlow>,
    pub source_element_ids: Vec<String>,
}

/// Why an import was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ImportError {
    /// A source element has no mapping.
    UnmappedElement { element_type: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnmappedElement { element_type } => {
                write!(f, "unmapped process element type {element_type}")
            }
        }
    }
}

impl std::error::Error for ImportError {}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-ORCH-R701.1
    #[test]
    fn imported_process_round_trips() {
        let process = ImportedProcess {
            schema_version: IMPORTED_PROCESS_SCHEMA_VERSION,
            model_id: "m1".into(),
            source_system: "aris".into(),
            steps: vec!["s1".into(), "s2".into()],
            gates: vec![
                ImportedGate { element_id: "g1".into(), kind: GateKind::And },
                ImportedGate { element_id: "g2".into(), kind: GateKind::Or },
                ImportedGate { element_id: "g3".into(), kind: GateKind::Xor },
            ],
            flows: vec![ImportedFlow { from: "s1".into(), to: "g1".into() }],
            source_element_ids: vec!["e1".into()],
        };
        let json = serde_json::to_value(&process).unwrap();
        assert_eq!(serde_json::from_value::<ImportedProcess>(json.clone()).unwrap(), process);
        let packed: ImportedProcess =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&process).unwrap()).unwrap();
        assert_eq!(packed, process);

        let mut bad = json.clone();
        bad["gates"][0]["kind"] = "nand".into();
        assert!(serde_json::from_value::<ImportedProcess>(bad).is_err());
        let mut extra = json;
        extra["extra"] = 1.into();
        assert!(serde_json::from_value::<ImportedProcess>(extra).is_err());

        let error = ImportError::UnmappedElement { element_type: "Swimlane".into() };
        let back: ImportError =
            serde_json::from_value(serde_json::to_value(&error).unwrap()).unwrap();
        assert_eq!(back, error);
    }
}
