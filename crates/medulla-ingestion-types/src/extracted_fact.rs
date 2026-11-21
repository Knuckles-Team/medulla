//! Typed fact and claim extraction records (MEDULLA-INGEST-WORKER-R701.1).
//!
//! A fact always carries an evidence span; a fact that grounds to no ontology
//! class is held as a candidate claim. Grounding, direction repair and
//! deduplication are later rows (R701.2 onward).

use serde::{Deserialize, Serialize};

/// The source span a fact or claim was read from. `end` is exclusive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "EvidenceSpanWire")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvidenceSpan {
    pub source_ref: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
struct EvidenceSpanWire {
    source_ref: String,
    start: u64,
    end: u64,
}

/// Why an evidence span is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceSpanError {
    #[error("evidence span source_ref must not be blank")]
    BlankSource,
    #[error("evidence span must be non-empty (start < end)")]
    EmptySpan,
}

impl EvidenceSpan {
    pub fn new(
        source_ref: impl Into<String>,
        start: u64,
        end: u64,
    ) -> Result<Self, EvidenceSpanError> {
        let source_ref = source_ref.into();
        if source_ref.trim().is_empty() {
            return Err(EvidenceSpanError::BlankSource);
        }
        if start >= end {
            return Err(EvidenceSpanError::EmptySpan);
        }
        Ok(Self {
            source_ref,
            start,
            end,
        })
    }
}

impl TryFrom<EvidenceSpanWire> for EvidenceSpan {
    type Error = EvidenceSpanError;

    fn try_from(wire: EvidenceSpanWire) -> Result<Self, Self::Error> {
        Self::new(wire.source_ref, wire.start, wire.end)
    }
}

/// The outcome of grounding a fact against the compiled ontology.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GroundingResult {
    Grounded {
        subject_class: String,
        property: String,
        object_class: String,
        /// True when subject and object were swapped to satisfy domain and range.
        direction_repaired: bool,
    },
    Ungrounded,
}

/// An atomic fact read from prose. The evidence span is mandatory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExtractedFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub evidence: EvidenceSpan,
    pub confidence: f64,
}

/// A statement held as a claim: extracted, evidenced, but not admitted as a
/// grounded fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CandidateClaim {
    pub fact: ExtractedFact,
    pub grounding: GroundingResult,
}

/// The payload of a `facts` source batch (MEDULLA-INGEST-WORKER-R701.3): the
/// prose facts the SDK extracted. The SDK decides prose or structured routing
/// and sends only prose facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FactBatch {
    pub facts: Vec<ExtractedFact>,
}

/// The most per-unit entries a [`FactReport`] holds.
pub const MAX_FACT_REPORT_ENTRIES: usize = 1024;

/// What one fact of a `facts` batch did to the stored claims
/// (MEDULLA-INGEST-WORKER-R701.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FactOutcome {
    /// No claim was stored under the claim id.
    Created,
    /// A claim was stored with a different unit hash: its confidence is combined.
    Updated,
    /// A claim was stored with the same unit hash: nothing is written.
    Unchanged,
    /// The fact carries no valid evidence span (MEDULLA-INGEST-WORKER-R701.4).
    Rejected,
}

/// The outcome of one fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FactUnit {
    /// The claim node id; empty for a rejected fact.
    pub claim_id: String,
    /// The hex unit hash: SHA-256 framed under `eg/extracted-fact` over subject,
    /// predicate, object and evidence span; empty for a rejected fact.
    pub unit_hash: String,
    pub outcome: FactOutcome,
    /// The refusal text of a rejected fact; empty otherwise.
    #[serde(default)]
    pub detail: String,
}

/// Per-outcome counts of a `facts` batch, the links it created and up to
/// [`MAX_FACT_REPORT_ENTRIES`] per-unit entries (MEDULLA-INGEST-WORKER-R701.5,
/// R701.7). Observation evidence: not part of the receipt digest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FactReport {
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub unchanged: u64,
    #[serde(default)]
    pub rejected: u64,
    /// `related_to` edges written between entities that share enough atoms
    /// (MEDULLA-INGEST-WORKER-R701.7).
    #[serde(default)]
    pub links: u64,
    #[serde(default)]
    pub entries: Vec<FactUnit>,
}

impl FactReport {
    /// Count one unit and keep its entry while the report has room.
    pub fn record(&mut self, unit: FactUnit) {
        match unit.outcome {
            FactOutcome::Created => self.created += 1,
            FactOutcome::Updated => self.updated += 1,
            FactOutcome::Unchanged => self.unchanged += 1,
            FactOutcome::Rejected => self.rejected += 1,
        }
        if self.entries.len() < MAX_FACT_REPORT_ENTRIES {
            self.entries.push(unit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn golden() -> serde_json::Value {
        json!({
            "fact": {
                "subject": "Ada Lovelace",
                "predicate": "wrote",
                "object": "Note G",
                "evidence": {"source_ref": "doc:ada", "start": 10, "end": 42},
                "confidence": 0.75
            },
            "grounding": {
                "status": "grounded",
                "subject_class": "Person",
                "property": "authorOf",
                "object_class": "Work",
                "direction_repaired": false
            }
        })
    }

    // spec: MEDULLA-INGEST-WORKER-R701.1
    #[test]
    fn golden_vector_round_trips_and_invalid_shapes_are_rejected() {
        let claim: CandidateClaim = serde_json::from_value(golden()).unwrap();
        assert_eq!(
            claim.fact.evidence,
            EvidenceSpan::new("doc:ada", 10, 42).unwrap()
        );
        assert_eq!(serde_json::to_value(&claim).unwrap(), golden());

        let ungrounded = CandidateClaim {
            grounding: GroundingResult::Ungrounded,
            ..claim.clone()
        };
        let encoded = serde_json::to_value(&ungrounded).unwrap();
        assert_eq!(encoded["grounding"], json!({"status": "ungrounded"}));
        assert_eq!(
            serde_json::from_value::<CandidateClaim>(encoded).unwrap(),
            ungrounded
        );

        // A fact without an evidence span is not representable.
        let mut no_span = golden();
        no_span["fact"].as_object_mut().unwrap().remove("evidence");
        assert!(serde_json::from_value::<CandidateClaim>(no_span).is_err());
        let mut empty_span = golden();
        empty_span["fact"]["evidence"]["end"] = json!(10);
        assert!(serde_json::from_value::<CandidateClaim>(empty_span).is_err());
        let mut blank_source = golden();
        blank_source["fact"]["evidence"]["source_ref"] = json!(" ");
        assert!(serde_json::from_value::<CandidateClaim>(blank_source).is_err());
        assert_eq!(
            EvidenceSpan::new("doc", 5, 5),
            Err(EvidenceSpanError::EmptySpan)
        );
    }
}
