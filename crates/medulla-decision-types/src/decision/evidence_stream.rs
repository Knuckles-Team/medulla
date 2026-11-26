//! Evidence-stream references sealed into decision records
//! (MEDULLA-DECISION-ENGINE-R019.100.1).
//!
//! A caller may name external evidence streams (a simulation, an inspection,
//! a body of process knowledge) a decision rests on. A stream is only
//! referenced, never read: the reference pins the stream id, the revision and
//! the content digest, the inputs digest covers it, and the decision's
//! premises record it as a request CLAIM, so the conclusion is classified by
//! the weakest thing it rests on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::record::premise::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_types::refusal_code::RefusalCode;

/// Most streams one request may name.
pub const MAX_EVIDENCE_STREAMS: usize = 16;
/// The request field the streams arrive in.
pub const EVIDENCE_STREAMS_FIELD: &str = "evidence_streams";

/// A `(kind, stream_id)` pair appears twice.
pub const EVIDENCE_STREAM_DUPLICATE: RefusalCode = RefusalCode::new("EVIDENCE_STREAM_DUPLICATE");
/// A digest is not `sha256:` plus 64 lowercase hex characters.
pub const EVIDENCE_STREAM_DIGEST_INVALID: RefusalCode =
    RefusalCode::new("EVIDENCE_STREAM_DIGEST_INVALID");

/// The refusal codes of the evidence-stream references.
pub const REFUSALS: &[RefusalCode] = &[EVIDENCE_STREAM_DIGEST_INVALID, EVIDENCE_STREAM_DUPLICATE];

/// What kind of evidence a stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EvidenceStreamKind {
    Simulation,
    Inspection,
    ProcessKnowledge,
}

impl EvidenceStreamKind {
    /// The kind's wire token, also the suffix of its premise fact.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Simulation => "simulation",
            Self::Inspection => "inspection",
            Self::ProcessKnowledge => "process_knowledge",
        }
    }
}

/// One referenced evidence stream, pinned at a revision and a content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvidenceStreamRef {
    pub kind: EvidenceStreamKind,
    pub stream_id: String,
    pub revision: u64,
    pub digest: String,
}

/// A typed refusal of the references: the code a caller branches on, and detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceStreamRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl EvidenceStreamRefusal {
    /// The served refusal text: `<CODE>: <detail>`.
    pub fn message(&self) -> String {
        format!("{}: {}", self.code.as_str(), self.detail)
    }
}

fn digest_is_valid(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Refuse a repeated `(kind, stream_id)` or a malformed digest. The digest is
/// checked first for each stream in order, so the first bad stream names the
/// refusal.
pub fn validate(streams: &[EvidenceStreamRef]) -> Result<(), EvidenceStreamRefusal> {
    let mut seen = BTreeSet::new();
    for stream in streams {
        if !digest_is_valid(&stream.digest) {
            return Err(EvidenceStreamRefusal {
                code: EVIDENCE_STREAM_DIGEST_INVALID,
                detail: format!(
                    "stream {} digest must be sha256: plus 64 lowercase hex characters",
                    stream.stream_id
                ),
            });
        }
        if !seen.insert((stream.kind, stream.stream_id.as_str())) {
            return Err(EvidenceStreamRefusal {
                code: EVIDENCE_STREAM_DUPLICATE,
                detail: format!(
                    "stream {} is named twice as {}",
                    stream.stream_id,
                    stream.kind.as_str()
                ),
            });
        }
    }
    Ok(())
}

/// One CLAIM premise per stream: `evidence_stream:<kind>`, provenance the
/// request field the stream arrived in.
pub fn premises(streams: &[EvidenceStreamRef]) -> Vec<PremiseRef> {
    streams
        .iter()
        .map(|stream| PremiseRef {
            subject: stream.stream_id.clone(),
            fact: format!("evidence_stream:{}", stream.kind.as_str()),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: EVIDENCE_STREAMS_FIELD.to_string(),
            },
            trust_tier: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(kind: EvidenceStreamKind, id: &str, digest: &str) -> EvidenceStreamRef {
        EvidenceStreamRef {
            kind,
            stream_id: id.to_string(),
            revision: 1,
            digest: digest.to_string(),
        }
    }

    fn good() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    // spec: MEDULLA-DECISION-ENGINE-R019.100.1
    #[test]
    fn digest_and_duplicate_rules() {
        let a = stream(EvidenceStreamKind::Simulation, "s1", &good());
        let b = stream(EvidenceStreamKind::Inspection, "s1", &good());
        assert!(validate(&[a.clone(), b]).is_ok(), "same id, other kind");
        assert_eq!(
            validate(&[a.clone(), a.clone()]).unwrap_err().code,
            EVIDENCE_STREAM_DUPLICATE
        );
        for bad in [
            "a".repeat(64),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "a".repeat(63)),
            format!("sha256:{}", "g".repeat(64)),
        ] {
            let refused = validate(&[stream(EvidenceStreamKind::Simulation, "s", &bad)]);
            assert_eq!(refused.unwrap_err().code, EVIDENCE_STREAM_DIGEST_INVALID);
        }
        let premises = premises(&[a]);
        assert_eq!(premises[0].fact, "evidence_stream:simulation");
        assert_eq!(premises[0].class, PremiseClass::Claim);
    }
}
