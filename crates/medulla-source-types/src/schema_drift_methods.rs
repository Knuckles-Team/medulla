//! The served schema drift records and repair approval (MEDULLA-TYPED-PACKS-R806).
//!
//! A delivery that breaks an approved contract is quarantined (R715). The quarantine node
//! keeps one [`DriftEvidence`] per drifted stream: the classification report and the inferred
//! record SHAPE (field names and JSON types, never a record value). The four served methods
//! read that evidence under the caller's tenant, derive the repair candidate from it, verify
//! the candidate against the graph without installing it, and activate it only on an approval
//! bound to the candidate digest.

use serde::{Deserialize, Serialize};

use medulla_catalog_types::graph_schema::drift::{DriftReport, InferredRecordShape};
use medulla_catalog_types::graph_schema::repair::RecordContract;
use medulla_types::contract::ResourceId;

/// The node type of a quarantined delivery (MEDULLA-INGEST-WORKER-R003.10.1).
pub const SOURCE_QUARANTINE_TYPE: &str = "SourceQuarantine";
/// The quarantine node property holding the [`DriftEvidenceRecord`] of a drift quarantine.
pub const DRIFT_EVIDENCE_PROPERTY: &str = "drift_evidence";

/// No quarantined drift of this tenant has that id (or names that stream). One body for an
/// unknown drift and another tenant's drift.
pub const DRIFT_NOT_FOUND: &str = "DRIFT_NOT_FOUND";
/// The approval is absent, still pending, refused, expired or binds another candidate.
pub const REPAIR_APPROVAL_MISSING: &str = "REPAIR_APPROVAL_MISSING";
/// The approved digest is not the digest of the candidate the drift derives.
pub const REPAIR_DIGEST_MISMATCH: &str = "REPAIR_DIGEST_MISMATCH";

/// Most drift records one list answers.
pub const MAX_DRIFT_LIST: usize = 500;

/// One drifted stream of a quarantined delivery: the classification and the inferred shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftEvidence {
    pub stream: ResourceId,
    pub report: DriftReport,
    pub shape: InferredRecordShape,
}

/// The stored value of the `drift_evidence` property of a quarantine node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftEvidenceRecord {
    pub connector: ResourceId,
    pub streams: Vec<DriftEvidence>,
}

/// One stream of a drift as the served reads show it: the report, never the shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftStreamView {
    pub stream: ResourceId,
    pub report: DriftReport,
}

/// One quarantined delivery with its drifted streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftRecordView {
    /// The id of the quarantine node.
    pub drift_id: String,
    pub connector: ResourceId,
    /// The delivery digest the quarantine was parked under.
    pub source_digest: String,
    pub streams: Vec<DriftStreamView>,
}

/// `SchemaDriftList`: the quarantined drifts of the caller's tenant.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaDriftListRequest {
    /// Keep only the drifts of this stream.
    #[serde(default)]
    pub stream: Option<ResourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaDriftListAnswer {
    pub drifts: Vec<DriftRecordView>,
}

/// `SchemaDriftGet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaDriftRef {
    pub drift_id: String,
}

/// `SchemaRepairPropose` and the target of an approval: one stream of one drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaRepairTarget {
    pub drift_id: String,
    pub stream: ResourceId,
}

/// The repair candidate a drift derives, with what the shadow check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaRepairPreview {
    pub drift_id: String,
    pub stream: ResourceId,
    /// The `approved:<stream>` key the candidate attaches under.
    pub source_id: String,
    pub candidate: RecordContract,
    /// The digest an approval grant must bind.
    pub candidate_digest: String,
    /// The composed schema digest after the shadow check: unchanged, nothing was applied.
    pub composed_digest: String,
    pub applied: bool,
}

/// `SchemaRepairApprove`: the approval receipt bound to the candidate digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaRepairApproveRequest {
    pub drift_id: String,
    pub stream: ResourceId,
    /// The candidate digest the approval binds; must be the digest the drift derives.
    pub proposal_digest: String,
    /// The consumed `action.approval` lease of the caller's tenant.
    pub approval_lease_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaRepairApproved {
    pub drift_id: String,
    pub stream: ResourceId,
    pub candidate_digest: String,
    /// False when the candidate was already the contract in force: nothing was written.
    pub activated: bool,
    pub composed_digest: String,
}

impl SchemaRepairApproveRequest {
    /// Refuse a request that names no approval before anything is read.
    pub fn validate(&self) -> Result<(), String> {
        if self.approval_lease_id.trim().is_empty() {
            return Err(format!(
                "{REPAIR_APPROVAL_MISSING}: an activation names its approval receipt"
            ));
        }
        medulla_catalog_types::graph_schema::approval::validate_approval_lease_id(
            &self.approval_lease_id,
        )
    }
}
