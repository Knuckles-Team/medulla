//! Process document records (MEDULLA-CQ-ORCH-R702.1).
//!
//! The engine reads BPMN 2.0 and DMN 1.3 documents natively. `GateKind` is
//! the one of the canonical workflow (MEDULLA-CQ-ORCH-R701.1).

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

pub use crate::process_import::GateKind;

/// Current record version of the process document types.
pub const PROCESS_EXCHANGE_SCHEMA_VERSION: u16 = 1;

fn schema_version() -> u16 {
    PROCESS_EXCHANGE_SCHEMA_VERSION
}

/// The document formats the engine reads natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProcessDocumentFormat {
    Bpmn20,
    Dmn13,
}

/// The record of the source a document came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceRecordRef {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub system: String,
    pub record_id: String,
}

/// One process document as a source hands it to the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
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

/// The answer of the served `ProcessDocumentImport` (MEDULLA-CQ-ORCH-R702.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProcessDocumentImported {
    /// The id of the stored model node, `processmodel:<model_iri>:v<version>`.
    pub model_node_id: String,
    /// The stored version of the model, 1 for the first import under the model IRI.
    pub version: u32,
    /// The document format that was read.
    pub format: ProcessDocumentFormat,
    /// The number of element nodes the batch upserted.
    pub nodes: u64,
    /// The number of edges the batch upserted.
    pub edges: u64,
    /// The SHA-256 of the input document bytes, 64 lowercase hex characters.
    pub source_document_digest: String,
}

/// What a `ProcessDocumentExport` writes a document from (MEDULLA-CQ-ORCH-R702.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExportSource {
    /// Version `version` of the imported model `model_iri`; the latest stored version when unset.
    /// Every version stays readable: a re-import writes a new one and changes no earlier one.
    Model {
        model_iri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
    },
    /// Version `version` of the stored canonical workflow `workflow_id`.
    Workflow { workflow_id: String, version: u32 },
}

/// The answer of the served `ProcessDocumentExport` (MEDULLA-CQ-ORCH-R702.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProcessDocumentExported {
    /// The written document; `source_ref` is unset.
    pub document: ProcessDocument,
    /// The SHA-256 of the written document bytes, 64 lowercase hex characters.
    pub document_digest: String,
    /// The canonical model digest, 64 lowercase hex characters (MEDULLA-CQ-ORCH-R702.13): the
    /// digest the model node carries as `canonicalModelDigest`, or the stored workflow's digest.
    pub canonical_model_digest: String,
}

/// The refusal codes of the served process document import and export (MEDULLA-CQ-ORCH-R702.12),
/// besides `PROCESS_EXPORT_NOT_DEPLOYABLE`, which `process_import::EXPORT_REFUSALS` declares.
/// The decision plane collects this table with every other family table and refuses a code
/// declared twice.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("PROCESS_DOC_MALFORMED"),
    RefusalCode::new("PROCESS_DOC_ENTITY_REFUSED"),
    RefusalCode::new("PROCESS_DOC_TOO_LARGE"),
    RefusalCode::new("PROCESS_DOC_NAMESPACE_UNKNOWN"),
    RefusalCode::new("PROCESS_DOC_RULE_ENTRIES_MISMATCH"),
];

/// Why a process document was not read or written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessExchangeError {
    Malformed { line: u64, column: u64 },
    EntityRefused,
    TooLarge { bytes: u64 },
    NamespaceUnknown { namespace: String },
    NotDeployable { reason: String },
    /// A decision-table rule whose entries do not match the table's clauses
    /// (MEDULLA-ORCH-VOCAB-R014.6).
    RuleEntriesMismatch {
        rule_id: String,
        input_entries: u64,
        input_clauses: u64,
        output_entries: u64,
        output_clauses: u64,
    },
}

impl ProcessExchangeError {
    /// The stable wire code of this refusal (MEDULLA-CQ-ORCH-R702.12).
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::Malformed { .. } => "PROCESS_DOC_MALFORMED",
            Self::EntityRefused => "PROCESS_DOC_ENTITY_REFUSED",
            Self::TooLarge { .. } => "PROCESS_DOC_TOO_LARGE",
            Self::NamespaceUnknown { .. } => "PROCESS_DOC_NAMESPACE_UNKNOWN",
            Self::NotDeployable { .. } => crate::process_import::PROCESS_EXPORT_NOT_DEPLOYABLE,
            Self::RuleEntriesMismatch { .. } => "PROCESS_DOC_RULE_ENTRIES_MISMATCH",
        }
    }
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
            Self::RuleEntriesMismatch {
                rule_id,
                input_entries,
                input_clauses,
                output_entries,
                output_clauses,
            } => write!(
                f,
                "rule {rule_id} has {input_entries} of {input_clauses} input entries and {output_entries} of {output_clauses} output entries"
            ),
        }
    }
}

impl std::error::Error for ProcessExchangeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: MEDULLA-CQ-ORCH-R702.12
    #[test]
    fn codes_are_unique_and_stable() {
        let mut codes: Vec<&str> = REFUSALS.iter().map(|c| c.as_str()).collect();
        let declared = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), declared, "a refusal code is declared twice");
        let errors = [
            ProcessExchangeError::Malformed { line: 1, column: 1 },
            ProcessExchangeError::EntityRefused,
            ProcessExchangeError::TooLarge { bytes: 1 },
            ProcessExchangeError::NamespaceUnknown {
                namespace: String::new(),
            },
            ProcessExchangeError::NotDeployable {
                reason: String::new(),
            },
            ProcessExchangeError::RuleEntriesMismatch {
                rule_id: String::new(),
                input_entries: 0,
                input_clauses: 1,
                output_entries: 0,
                output_clauses: 1,
            },
        ];
        for error in &errors {
            let code = error.wire_code();
            assert!(
                codes.contains(&code)
                    || crate::process_import::EXPORT_REFUSALS.contains(&code),
                "{code} is not declared"
            );
        }
    }

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
