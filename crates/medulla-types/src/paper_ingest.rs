//! Paper ingest refusals and the per-batch paper report (MEDULLA-MEDIA-VOCAB-R002.5.1 to
//! R002.5.5).
//!
//! A `SourceIngest` record whose mapped class is a research paper is checked before any of its
//! nodes are admitted; a refused paper writes nothing and its siblings in the envelope still
//! commit. Collected by `medulla_decision_types::decision::codes::module_tables`
//! (MEDULLA-DECISION-ENGINE-R010.100), so no code here repeats a shared or foreign code.

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

/// The paper's blank canonical id, empty or blank references, or an `id` that is not the digest of
/// its identity.
pub const PAPER_IDENTITY_INVALID: RefusalCode = RefusalCode::new("PAPER_IDENTITY_INVALID");
/// A document receipt of another tenant, paper, revision or a malformed digest, size or type.
pub const PAPER_RECEIPT_MISMATCH: RefusalCode = RefusalCode::new("PAPER_RECEIPT_MISMATCH");
/// A receipt artifact that is not a relative path, or a cited document with no matching receipt.
pub const PAPER_ARTIFACT_UNAVAILABLE: RefusalCode = RefusalCode::new("PAPER_ARTIFACT_UNAVAILABLE");
/// A populated title, authors, abstract or publication date with no field provenance.
pub const PAPER_FIELD_PROVENANCE_MISSING: RefusalCode =
    RefusalCode::new("PAPER_FIELD_PROVENANCE_MISSING");

/// Every refusal code of the paper ingest family.
pub const REFUSALS: &[RefusalCode] = &[
    PAPER_IDENTITY_INVALID,
    PAPER_RECEIPT_MISMATCH,
    PAPER_ARTIFACT_UNAVAILABLE,
    PAPER_FIELD_PROVENANCE_MISSING,
];

/// One refused paper of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PaperRefusal {
    /// The source record id of the refused paper.
    pub record_id: String,
    /// One of [`REFUSALS`].
    pub code: String,
}

/// The papers a `SourceIngest` batch refused. Observation evidence: not part of the receipt digest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PaperReport {
    #[serde(default)]
    pub refusals: Vec<PaperRefusal>,
}
