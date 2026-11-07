//! Process document records (MEDULLA-CQ-ORCH-R702.1).
//!
//! The engine reads BPMN 2.0 and DMN 1.3 documents natively. `GateKind` is
//! the one of the canonical workflow (MEDULLA-CQ-ORCH-R701.1).

use serde::{Deserialize, Serialize};

pub use crate::process_import::GateKind;

/// Current record version of the process document types.
pub const PROCESS_EXCHANGE_SCHEMA_VERSION: u16 = 1;

fn schema_version() -> u16 {
    PROCESS_EXCHANGE_SCHEMA_VERSION
}

/// The document formats the engine reads natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessDocumentFormat {
    Bpmn20,
    Dmn13,
}

/// The record of the source a document came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecordRef {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub system: String,
    pub record_id: String,
}

/// One process document as a source hands it to the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessDocument {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub format: ProcessDocumentFormat,
    #[serde(with = "serde_bytes")]
    pub document: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<SourceRecordRef>,
}

/// Why a process document was not read or written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessExchangeError {
    Malformed { line: u64, column: u64 },
    EntityRefused,
    TooLarge { bytes: u64 },
    NamespaceUnknown { namespace: String },
    NotDeployable { reason: String },
}

impl std::fmt::Display for ProcessExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed { line, column } => {
                write!(f, "process document is malformed at {line}:{column}")
            }
            Self::EntityRefused => write!(f, "process document declares an entity"),
            Self::TooLarge { bytes } => write!(f, "process document exceeds {bytes} bytes"),
            Self::NamespaceUnknown { namespace } => {
                write!(f, "process document namespace {namespace} is unknown")
            }
            Self::NotDeployable { reason } => write!(f, "workflow is not deployable: {reason}"),
        }
    }
}

impl std::error::Error for ProcessExchangeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: MEDULLA-CQ-ORCH-R702.1
    #[test]
    fn document_records_round_trip() {
        let doc = ProcessDocument {
            schema_version: PROCESS_EXCHANGE_SCHEMA_VERSION,
            format: ProcessDocumentFormat::Bpmn20,
            document: b"<definitions/>".to_vec(),
            source_ref: Some(SourceRecordRef {
                schema_version: 1,
                system: "process-engine".into(),
                record_id: "invoice:3".into(),
            }),
        };
        let encoded = serde_json::to_value(&doc).unwrap();
        assert_eq!(
            serde_json::from_value::<ProcessDocument>(encoded.clone()).unwrap(),
            doc
        );
        let mut unknown = encoded;
        unknown["format"] = json!("xpdl");
        assert!(serde_json::from_value::<ProcessDocument>(unknown).is_err());
        let error = ProcessExchangeError::Malformed { line: 3, column: 7 };
        let wire = serde_json::to_value(&error).unwrap();
        assert_eq!(wire["code"], "malformed");
        assert_eq!(
            serde_json::from_value::<ProcessExchangeError>(wire).unwrap(),
            error
        );
    }
}
