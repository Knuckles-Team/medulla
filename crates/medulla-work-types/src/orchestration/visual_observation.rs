//! A run-bound visual observation record (MEDULLA-ORCH-VOCAB-R810).
//!
//! One observation is the envelope of a connector tool's visual output: the payload stays behind
//! `content_ref`, only its digest and the identifying fields are stored. The field set is the
//! connector-control envelope of SCION-CONNECTOR-CONTROL-R052.1 plus a retention mark and an
//! invalidation mark. The record is plain data: the serde types are the wire source, and the
//! write is one `upsert_node` batch through the graph gateway.

use serde::{Deserialize, Serialize};

/// The node type an observation is stored under.
pub const VISUAL_OBSERVATION_NODE_TYPE: &str = "VisualObservation";
/// The property that holds the canonical JSON of the stored record.
pub const VISUAL_OBSERVATION_RECORD_PROPERTY: &str = "observation_record";
/// The serialized envelope bound (8 KiB).
pub const MAX_ENVELOPE_BYTES: usize = 8 * 1024;

/// How long the observation is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetentionMark {
    /// Epoch milliseconds after which the observation may be removed.
    pub retain_until_ms: u64,
}

/// Why and when the observation stopped being valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InvalidationMark {
    pub invalidated_at_ms: u64,
    pub reason: String,
}

/// One run-bound visual observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VisualObservation {
    pub observation_id: String,
    pub modality: String,
    pub source_operation_id: String,
    /// The frame the observation shows; one of `frame_id` and `sequence` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// RFC 3339 capture time.
    pub captured_at: String,
    /// 64 lowercase hex characters: the SHA-256 of the payload behind `content_ref`.
    pub content_digest: String,
    pub content_ref: String,
    pub tenant_id: String,
    pub run_id: String,
    pub request_id: String,
    pub schema_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation: Option<InvalidationMark>,
}

/// What `PutObservation` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VisualObservationReceipt {
    pub node_id: String,
    /// True when the stored record already held exactly this observation.
    pub replayed: bool,
}

/// The typed refusals of an observation write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationRefusal {
    /// A field is empty, malformed, or the envelope exceeds its bound.
    Invalid(String),
    /// The record names a tenant other than the verified caller's, or a run the caller's tenant
    /// does not hold. Both read the same: no existence oracle.
    ScopeUnknown,
    /// The observation id is stored with a different content digest.
    Conflict,
}

impl ObservationRefusal {
    /// The wire code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "OBSERVATION_INVALID",
            Self::ScopeUnknown => "OBSERVATION_SCOPE_UNKNOWN",
            Self::Conflict => "OBSERVATION_CONFLICT",
        }
    }
}

impl std::fmt::Display for ObservationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "{}: {what}", self.code()),
            Self::ScopeUnknown => write!(f, "{}: no such run for the caller", self.code()),
            Self::Conflict => write!(
                f,
                "{}: the observation id is stored with another content digest",
                self.code()
            ),
        }
    }
}

/// The id of the node that stores `observation_id` of `run_id`.
pub fn observation_node_id(run_id: &str, observation_id: &str) -> String {
    format!("observation:{run_id}:{observation_id}")
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl VisualObservation {
    /// The id of the node that stores this observation.
    pub fn node_id(&self) -> String {
        observation_node_id(&self.run_id, &self.observation_id)
    }

    /// Check the record against its own bounds. Scope (tenant and run) is checked by the writer
    /// against the verified caller, not here.
    pub fn validate(&self) -> Result<(), ObservationRefusal> {
        let invalid = |what: &str| Err(ObservationRefusal::Invalid(what.to_string()));
        for (name, value) in [
            ("observation_id", &self.observation_id),
            ("modality", &self.modality),
            ("source_operation_id", &self.source_operation_id),
            ("captured_at", &self.captured_at),
            ("content_ref", &self.content_ref),
            ("tenant_id", &self.tenant_id),
            ("run_id", &self.run_id),
            ("request_id", &self.request_id),
        ] {
            if value.trim().is_empty() {
                return invalid(&format!("{name} is empty"));
            }
        }
        if self.frame_id.as_deref().is_none_or(str::is_empty) == self.sequence.is_none() {
            return invalid("exactly one of frame_id and sequence names the frame");
        }
        if !is_digest(&self.content_digest) {
            return invalid("content_digest must be 64 lowercase hexadecimal characters");
        }
        if let Some(mark) = &self.invalidation {
            if mark.reason.trim().is_empty() {
                return invalid("an invalidation names its reason");
            }
        }
        let size = serde_json::to_vec(self)
            .map_err(|error| ObservationRefusal::Invalid(error.to_string()))?
            .len();
        if size > MAX_ENVELOPE_BYTES {
            return invalid("the envelope exceeds 8 KiB serialized");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn observation() -> VisualObservation {
        VisualObservation {
            observation_id: "obs-1".into(),
            modality: "screenshot".into(),
            source_operation_id: "browser.capture".into(),
            frame_id: Some("frame-1".into()),
            sequence: None,
            captured_at: "2026-11-26T10:00:00Z".into(),
            content_digest: "ab".repeat(32),
            content_ref: "blob://observations/obs-1".into(),
            tenant_id: "tenant-a".into(),
            run_id: "run-1".into(),
            request_id: "req-1".into(),
            schema_revision: 1,
            retention: Some(RetentionMark {
                retain_until_ms: 1_800_000_000_000,
            }),
            invalidation: None,
        }
    }

    #[test]
    fn envelope_round_trips_and_bounds() {
        let record = observation();
        record.validate().expect("a valid envelope");
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<VisualObservation>(&json).unwrap(),
            record
        );
        let mut both = record.clone();
        both.sequence = Some(3);
        assert!(both.validate().is_err(), "frame_id and sequence together");
        let mut neither = record.clone();
        neither.frame_id = None;
        assert!(neither.validate().is_err());
        let mut digest = record.clone();
        digest.content_digest = "AB".repeat(32);
        assert!(digest.validate().is_err());
        let mut large = record;
        large.content_ref = "x".repeat(MAX_ENVELOPE_BYTES);
        assert!(large.validate().is_err());
    }
}
