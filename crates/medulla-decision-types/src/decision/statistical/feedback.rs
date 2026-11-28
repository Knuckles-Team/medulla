//! The feedback event of a retrieval-plan record (MEDULLA-DECISION-ENGINE-R100.1.1).
//!
//! A caller reports how a committed retrieval run turned out for its user: the
//! run succeeded, failed or was corrected. The event joins one record by id and
//! digest and names the evidence units it concerns. It is written once per
//! `(event id, source revision)` through `DecisionLog` `learn`.

use serde::{Deserialize, Serialize};

use crate::decision::context_package::is_sha256_hex;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// The schema version of a feedback event.
pub const FEEDBACK_SCHEMA_VERSION: u16 = 1;
/// Most items either evidence list holds.
pub const MAX_FEEDBACK_EVIDENCE: usize = 64;
/// Longest event id, in bytes.
pub const MAX_EVENT_ID_BYTES: usize = 128;
/// The key prefix of stored feedback events.
pub const FEEDBACK_PREFIX: &str = "feedback/";

/// The record, option or outcome the event joins is missing.
pub const FEEDBACK_JOIN_INCOMPLETE: RefusalCode = RefusalCode::new("FEEDBACK_JOIN_INCOMPLETE");
/// The record digest or an evidence id differs from the stored record.
pub const FEEDBACK_EVIDENCE_MISMATCH: RefusalCode = RefusalCode::new("FEEDBACK_EVIDENCE_MISMATCH");
/// A stored revision of the event id is higher than this one.
pub const FEEDBACK_SOURCE_STALE: RefusalCode = RefusalCode::new("FEEDBACK_SOURCE_STALE");

/// The feedback refusals, in the `RetrievalPlan` table.
pub const REFUSALS: &[RefusalCode] = &[
    FEEDBACK_EVIDENCE_MISMATCH,
    FEEDBACK_JOIN_INCOMPLETE,
    FEEDBACK_SOURCE_STALE,
];

/// The `feedback.*` knobs: `max_evidence`.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "feedback.max_evidence",
    unit: "items",
    default: KnobValue::Integer(MAX_FEEDBACK_EVIDENCE as i64),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(MAX_FEEDBACK_EVIDENCE as i64),
    safety: KnobSafety::Tunable,
}];

/// How the run turned out for its user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FeedbackOutcome {
    Ok,
    Error,
    Correction,
}

/// One feedback event on a committed retrieval-plan record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeedbackEvent {
    pub schema_version: u16,
    /// 1 to 128 bytes.
    pub event_id: String,
    /// At least 1: the revision of the source the event was read at.
    pub source_revision: u64,
    pub record_id: String,
    /// SHA-256 of the record, 64 lowercase hex characters.
    pub record_digest: String,
    pub option_id: String,
    pub actor_id: String,
    pub outcome: FeedbackOutcome,
    pub evidence_ids: BoundedVec<String, MAX_FEEDBACK_EVIDENCE>,
    /// One digest per evidence id, same length, 64 lowercase hex characters each.
    pub evidence_digests: BoundedVec<String, MAX_FEEDBACK_EVIDENCE>,
    pub event_time_ms: u64,
}

impl FeedbackEvent {
    /// The shape every event must have before it is joined; `max_evidence` is
    /// the resolved `feedback.max_evidence` knob.
    pub fn check(&self, max_evidence: usize) -> Result<(), String> {
        if self.schema_version != FEEDBACK_SCHEMA_VERSION {
            return Err(format!(
                "feedback schema_version must be {FEEDBACK_SCHEMA_VERSION}"
            ));
        }
        if self.event_id.is_empty() || self.event_id.len() > MAX_EVENT_ID_BYTES {
            return Err(format!("event_id must be 1 to {MAX_EVENT_ID_BYTES} bytes"));
        }
        if self.event_id.contains('/') {
            return Err("event_id must not contain '/'".to_string());
        }
        if self.source_revision == 0 {
            return Err("source_revision must be at least 1".to_string());
        }
        if !is_sha256_hex(&self.record_digest) {
            return Err("record_digest must be 64 lowercase hex characters".to_string());
        }
        if self.evidence_ids.len() != self.evidence_digests.len() {
            return Err("evidence_ids and evidence_digests must have the same length".to_string());
        }
        if self.evidence_ids.len() > max_evidence {
            return Err(format!(
                "at most {max_evidence} evidence items are allowed, got {}",
                self.evidence_ids.len()
            ));
        }
        if let Some(bad) = self
            .evidence_digests
            .iter()
            .find(|digest| !is_sha256_hex(digest))
        {
            return Err(format!(
                "evidence digest {bad:?} is not 64 lowercase hex characters"
            ));
        }
        Ok(())
    }
}

/// The stored feedback event: the event, who recorded it and when, and the
/// premises that name it (`feedback:event_id`, `feedback:source_revision` and
/// `feedback:record_digest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StoredFeedback {
    pub event: FeedbackEvent,
    pub producer: String,
    pub recorded_at_ms: u64,
    pub premises: Vec<String>,
}

/// The key of one stored event.
pub fn feedback_key(event_id: &str, source_revision: u64) -> String {
    format!("{FEEDBACK_PREFIX}{event_id}/{source_revision}")
}

/// The key prefix of every stored revision of one event id.
pub fn feedback_prefix(event_id: &str) -> String {
    format!("{FEEDBACK_PREFIX}{event_id}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> FeedbackEvent {
        FeedbackEvent {
            schema_version: FEEDBACK_SCHEMA_VERSION,
            event_id: "ev-1".into(),
            source_revision: 2,
            record_id: "rec-1".into(),
            record_digest: "a".repeat(64),
            option_id: "plan-a".into(),
            actor_id: "user-1".into(),
            outcome: FeedbackOutcome::Correction,
            evidence_ids: BoundedVec::new(vec!["u1".into(), "u2".into()]).unwrap(),
            evidence_digests: BoundedVec::new(vec!["b".repeat(64), "c".repeat(64)]).unwrap(),
            event_time_ms: 9,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.1.1
    #[test]
    fn feedback_event_round_trips() {
        let event = event();
        event.check(MAX_FEEDBACK_EVIDENCE).unwrap();
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<FeedbackEvent>(&text).unwrap(), event);
        let extra = text.replacen('{', r#"{"extra":1,"#, 1);
        assert!(serde_json::from_str::<FeedbackEvent>(&extra).is_err());
        assert_eq!(feedback_key("ev-1", 2), "feedback/ev-1/2");

        let mut short = event.clone();
        short.evidence_digests = BoundedVec::new(vec!["b".repeat(64)]).unwrap();
        assert!(short.check(MAX_FEEDBACK_EVIDENCE).is_err());
        let mut revision = event.clone();
        revision.source_revision = 0;
        assert!(revision.check(MAX_FEEDBACK_EVIDENCE).is_err());
        assert!(
            event.check(1).is_err(),
            "the knob bounds the evidence lists"
        );
    }
}
