//! Types of the served paper-admission family (MEDULLA-DECISION-ENGINE-R031.100).
//!
//! Staged paper nodes in a graph are admitted when the pinned head's calibrated
//! relevance reaches `enrichment.relevance_min`. The refusals, the knob and the
//! parameter names the family and its callers share live here.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::{DecisionRefusal, RefusalCode};

/// Text parameter naming the platform goals the BM25 feature reads.
pub const PARAM_GOALS: &str = "platform.goals";
/// Integer parameter: the graph version the staged snapshot was taken at.
pub const PARAM_GRAPH_VERSION: &str = "graph.version";
/// Integer parameter the server fills with the version of the graph it read.
pub const PARAM_OBSERVED_VERSION: &str = "paper.observed_graph_version";
/// Candidate text: the paper's content digest.
pub const TEXT_CONTENT_DIGEST: &str = "paper.content_digest";
/// Candidate text: the paper's claim ids, comma separated.
pub const TEXT_CLAIM_IDS: &str = "paper.claim_ids";

/// The `enrichment.*` decision-policy knobs of paper admission.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "enrichment.relevance_min",
    unit: "probability",
    default: KnobValue::DecimalMicros(600_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
}];

/// The least calibrated relevance, in millionths, that admits a paper.
pub fn relevance_min_micros(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DecimalMicros(v) => v,
        _ => 600_000,
    }
}

/// The name of the parameter the server fills with the nearest-neighbour
/// similarities (candidate id to `Q32`, JSON) of the feature naming `graph_param`.
pub fn nearest_neighbour_param(graph_param: &str) -> String {
    format!("nearest_neighbour:{graph_param}")
}

/// Why a paper batch is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaperAdmissionRefusal {
    /// The graph moved since the snapshot was staged.
    SnapshotStale { staged: i64, observed: i64 },
    /// Two staged papers share a content digest.
    Duplicate { digest: String },
}

/// The family's refusal table, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("PAPER_SNAPSHOT_STALE"),
    RefusalCode::new("PAPER_DUPLICATE"),
];

impl DecisionRefusal for PaperAdmissionRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::SnapshotStale { .. } => REFUSALS[0],
            Self::Duplicate { .. } => REFUSALS[1],
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::SnapshotStale { staged, observed } => format!(
                "the papers were staged at graph version {staged} but the graph is at {observed}"
            ),
            Self::Duplicate { digest } => {
                format!("more than one staged paper has the content digest {digest}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R031.100
    #[test]
    fn refusals_have_registered_codes_and_the_knob_defaults() {
        let stale = PaperAdmissionRefusal::SnapshotStale {
            staged: 1,
            observed: 2,
        };
        assert_eq!(stale.code().as_str(), "PAPER_SNAPSHOT_STALE");
        let dup = PaperAdmissionRefusal::Duplicate { digest: "d".into() };
        assert_eq!(dup.code().as_str(), "PAPER_DUPLICATE");
        assert_eq!(
            relevance_min_micros(&DecisionPolicy::engine_default()),
            600_000
        );
        assert_eq!(
            nearest_neighbour_param("platform.graph"),
            "nearest_neighbour:platform.graph"
        );
    }
}
