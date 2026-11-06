//! Failure-pattern mining types (MEDULLA-DECISION-ENGINE-R701.1).
//!
//! A failed run becomes a [`FailureRecord`]; records sharing a normalised
//! [`Signature`] cluster into a [`FailurePattern`]; a pattern seen often
//! enough files one [`GapTopic`], keyed by the SHA-256 of its signature so a
//! repeat ingest files no second gap. [`FailureOp`] is the body of the served
//! `Method::FailureIngest`. The clustering kernels live in
//! `medulla-epistemic::failure_patterns`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contract::BoundedVec;

/// Most steps one failure record or pattern carries.
pub const MAX_FAILURE_STEPS: usize = 256;
/// Most member records one pattern lists.
pub const MAX_PATTERN_MEMBERS: usize = 1024;

/// One failed run, read from a `RunTrace` node (or a `HealthAnomaly` node).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecord {
    /// Stable id of this record.
    pub record_id: String,
    /// Id of the run-trace node the record came from.
    pub run_trace_id: String,
    /// The run's error class (its `terminationReason`).
    pub error_class: String,
    /// The tool the failing step used (`usesTool`).
    pub tool: String,
    /// The kind of the failing step.
    pub step_kind: String,
    /// The run's steps in `precedes` order.
    pub steps: BoundedVec<String, MAX_FAILURE_STEPS>,
    /// Index of the failing step within `steps`.
    pub failing_index: u32,
    /// When the run failed, Unix milliseconds.
    pub at_ms: u64,
}

/// A normalised failure signature: `error_class|tool|step_kind`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Signature(pub String);

impl Signature {
    /// The id of the gap topic this signature files: SHA-256 of the
    /// signature as 64 lowercase hex characters.
    pub fn topic_id(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// Records that share one signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailurePattern {
    /// The shared signature.
    pub signature: Signature,
    /// Member record ids.
    pub members: BoundedVec<String, MAX_PATTERN_MEMBERS>,
    /// Earliest member failure, Unix milliseconds.
    pub first_seen: u64,
    /// Latest member failure, Unix milliseconds.
    pub last_seen: u64,
    /// Number of member records.
    pub count: u64,
    /// The failing step and the context steps before it.
    pub minimal_steps: BoundedVec<String, MAX_FAILURE_STEPS>,
}

/// Whether a gap is still open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum GapStatus {
    /// Nothing addresses the gap yet.
    Open,
    /// A source node covers the gap.
    Addressed {
        /// The node that addresses it.
        by: String,
    },
}

/// The gap a recurring pattern files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapTopic {
    /// SHA-256 of `signature`, 64 lowercase hex characters.
    pub id: String,
    /// The pattern signature.
    pub signature: Signature,
    /// Open or addressed.
    pub status: GapStatus,
}

impl GapTopic {
    /// An open topic for `signature`, its id derived from the signature.
    pub fn open(signature: Signature) -> Self {
        Self {
            id: signature.topic_id(),
            signature,
            status: GapStatus::Open,
        }
    }
}

/// The body of `Method::FailureIngest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum FailureOp {
    /// Read failed runs, cluster them and file gaps for recurring patterns.
    Ingest {
        /// Only runs that failed at or after this time, Unix milliseconds.
        #[serde(default)]
        since_ms: Option<u64>,
    },
    /// Mark a topic addressed by a source node that `COVERS` it.
    MarkAddressed {
        /// The topic id.
        topic_id: String,
        /// The covering source node.
        by: String,
    },
    /// Read one gap with its member run ids.
    Get {
        /// The topic id.
        topic_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_RECORD: &str = r#"{"record_id":"r1","run_trace_id":"rt1","error_class":"max_steps","tool":"search","step_kind":"tool_call","steps":["plan","search"],"failing_index":1,"at_ms":1700000000000}"#;
    const GOLDEN_TOPIC: &str = r#"{"id":"80b3386c082de9fd7bf95d09d9f30dfffc828b3e23ace5c9e18db0d269d28c5f","signature":"max_steps|search|tool_call","status":{"state":"addressed","by":"doc:1"}}"#;

    // spec: MEDULLA-DECISION-ENGINE-R701.1
    #[test]
    fn failure_types_round_trip_with_golden_vector() {
        let record = FailureRecord {
            record_id: "r1".into(),
            run_trace_id: "rt1".into(),
            error_class: "max_steps".into(),
            tool: "search".into(),
            step_kind: "tool_call".into(),
            steps: BoundedVec::new(vec!["plan".into(), "search".into()]).unwrap(),
            failing_index: 1,
            at_ms: 1_700_000_000_000,
        };
        assert_eq!(serde_json::to_string(&record).unwrap(), GOLDEN_RECORD);
        assert_eq!(
            serde_json::from_str::<FailureRecord>(GOLDEN_RECORD).unwrap(),
            record
        );

        let signature = Signature("max_steps|search|tool_call".into());
        let id = signature.topic_id();
        assert_eq!(id.len(), 64);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let topic = GapTopic {
            status: GapStatus::Addressed { by: "doc:1".into() },
            ..GapTopic::open(signature)
        };
        assert_eq!(
            id,
            "80b3386c082de9fd7bf95d09d9f30dfffc828b3e23ace5c9e18db0d269d28c5f"
        );
        let golden_topic = GOLDEN_TOPIC;
        assert_eq!(serde_json::to_string(&topic).unwrap(), golden_topic);
        assert_eq!(
            serde_json::from_str::<GapTopic>(golden_topic).unwrap(),
            topic
        );

        let pattern = FailurePattern {
            signature: Signature("a|b|c".into()),
            members: BoundedVec::new(vec!["r1".into(), "r2".into()]).unwrap(),
            first_seen: 1,
            last_seen: 2,
            count: 2,
            minimal_steps: BoundedVec::new(vec!["s".into()]).unwrap(),
        };
        let json = serde_json::to_string(&pattern).unwrap();
        assert_eq!(
            serde_json::from_str::<FailurePattern>(&json).unwrap(),
            pattern
        );

        for op in [
            FailureOp::Ingest { since_ms: Some(5) },
            FailureOp::MarkAddressed {
                topic_id: id.clone(),
                by: "doc:1".into(),
            },
            FailureOp::Get { topic_id: id },
        ] {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(serde_json::from_str::<FailureOp>(&json).unwrap(), op);
        }

        // deny_unknown_fields and the step bound both refuse on decode.
        let extra = GOLDEN_RECORD.replacen('{', r#"{"extra":1,"#, 1);
        assert!(serde_json::from_str::<FailureRecord>(&extra).is_err());
        let too_many = vec!["s"; MAX_FAILURE_STEPS + 1];
        let over = format!(
            r#"{{"record_id":"r","run_trace_id":"t","error_class":"e","tool":"x","step_kind":"k","steps":{},"failing_index":0,"at_ms":0}}"#,
            serde_json::to_string(&too_many).unwrap()
        );
        assert!(serde_json::from_str::<FailureRecord>(&over).is_err());
    }
}
