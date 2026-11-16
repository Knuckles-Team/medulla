//! MEDULLA-UNIFIED-DATA-PLANE-R760 — the served incident records.
//!
//! The wire shapes of the five incident methods (`IncidentRecordAnomaly`,
//! `IncidentCorrelate`, `IncidentEvidence`, `IncidentRelatedClaims`,
//! `IncidentSetStatus`) and the record the incident authority keeps. The
//! tenant of every record comes from the verified request carrier, never from
//! a request body.

use serde::{Deserialize, Serialize};

/// One anomaly reading as recorded through `IncidentRecordAnomaly`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AnomalyReading {
    /// The resource the anomaly is about; required (`ANOMALY_RESOURCE_REQUIRED`).
    pub entity_ref: String,
    pub signal: String,
    pub observed: f64,
    pub baseline: f64,
    pub at_ms: u64,
}

/// Incident severity, from the largest relative deviation in its evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IncidentSeverity {
    Low,
    Medium,
    High,
}

/// Incident status; it only moves forward one step at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IncidentStatus {
    Open,
    Acknowledged,
    Resolved,
}

impl IncidentStatus {
    /// The one status this status may move to, if any.
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Open => Some(Self::Acknowledged),
            Self::Acknowledged => Some(Self::Resolved),
            Self::Resolved => None,
        }
    }
}

/// One served incident and its evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IncidentRecord {
    pub id: String,
    pub tenant: String,
    /// SHA-256 of the signature, 64 lowercase hex characters.
    pub signature_digest: String,
    pub severity: IncidentSeverity,
    pub status: IncidentStatus,
    pub opened_at_ms: u64,
    pub evidence: Vec<AnomalyReading>,
    pub status_history: Vec<IncidentStatus>,
}

/// What one incident write touched.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IncidentWriteReceipt {
    /// The incidents the call opened, joined or moved, in id order.
    pub incident_ids: Vec<String>,
    /// Incident and `HealthAnomaly` evidence nodes committed in the one batch.
    pub nodes_written: u64,
}

/// One `Claim` node about an entity in an incident's evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IncidentRelatedClaim {
    pub claim_id: String,
    /// The evidence `entity_ref` the claim's `about` edge points at.
    pub entity_ref: String,
    pub properties: serde_json::Value,
}
