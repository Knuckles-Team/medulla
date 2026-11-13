//! The query-side embedding adapter (EH-396): a small fitted statistical head,
//! never a mutation of stored vectors.
//!
//! An adapter is `r <= 8` orthonormal directions `u_i` with bounded gains
//! `g_i` (`|g_i| <= 1/2`) in ONE embedding space. It maps a query vector
//! `q -> q + sum_i g_i (u_i . q) u_i` -- the identity plus a symmetric low-rank
//! term -- and is applied to the QUERY only, inside the ANN probe, after row-level
//! security has fixed the candidate set. Corpus vectors are never touched, so
//! activating or rolling back an adapter is a pointer move, every prior state
//! replays, and an adapter can only re-order rows the caller could already see.
//!
//! Every number is fixed-point: unit components on `Q30`, gains on `Q16`, so a
//! body and its digest are identical on every release target.

use serde::{Deserialize, Serialize};

use super::super::jobs::RecordWindow;
use super::retrieval::MAX_QUERY_DIMENSIONS;
use medulla_types::contract::BoundedVec;

/// Format identity of an adapter body.
pub const QUERY_ADAPTER_SCHEMA_VERSION: u16 = 1;
/// Most directions an adapter may carry.
pub const MAX_ADAPTER_RANK: usize = 8;
/// Largest admissible `|g_i|`, on `Q16` (one half).
pub const MAX_ADAPTER_GAIN_Q16: i32 = 1 << 15;
/// The `Q30` scale of a unit component.
pub const UNIT_SCALE_BITS: u32 = 30;
/// Tolerance, on `Q30`, of the orthonormality check (about 1e-3).
const ORTHONORMAL_TOLERANCE_Q30: i128 = 1 << 20;

/// One direction and its gain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterDirection {
    pub gain_q16: i32,
    pub unit_q30: BoundedVec<i32, MAX_QUERY_DIMENSIONS>,
}

/// What an adapter is fitted for. Closed: an unknown scope is refused by the
/// wire decoder (MEDULLA-DECISION-ENGINE-R174.7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AdapterScope {
    /// Every query against the graph.
    Graph,
    /// Queries over one corpus.
    Corpus { corpus_iri: String },
    /// Queries made for one capability.
    Capability { capability_iri: String },
}

impl AdapterScope {
    /// The stable key fragment of a scope, used in pointer keys.
    pub fn key(&self) -> String {
        match self {
            Self::Graph => "graph".to_string(),
            Self::Corpus { corpus_iri } => format!("corpus:{corpus_iri}"),
            Self::Capability { capability_iri } => format!("capability:{capability_iri}"),
        }
    }
}

/// Which side of retrieval an adapter transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AdapterSide {
    /// The query vector, at read.
    Query,
    /// The stored vectors, at read, with no embedding endpoint call.
    Index,
}

/// Which recorded outcomes train an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum OutcomeSource {
    RetrievalOutcome,
    RunOutcome,
    CapabilityOutcome,
}

/// The body of a fitted query adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryAdapterBody {
    pub schema_version: u16,
    /// The `EmbeddingSpaceRef::digest` it applies in; any other space ignores it.
    pub space_digest: String,
    pub dimensions: u32,
    pub directions: BoundedVec<AdapterDirection, MAX_ADAPTER_RANK>,
    /// Digest of the admitted training items, in order.
    pub training_digest: String,
    pub n_training: u64,
    /// Refit counter within one scope; the first fit is revision 1.
    pub revision: u32,
    /// Digest of the adapter this one was refitted from.
    pub parent_digest: Option<String>,
}

fn dot_q60(left: &[i32], right: &[i32]) -> i128 {
    left.iter()
        .zip(right)
        .map(|(a, b)| i128::from(*a) * i128::from(*b))
        .sum()
}

fn near(value_q60: i128, target_q30: i128) -> bool {
    let value_q30 = value_q60 >> UNIT_SCALE_BITS;
    (value_q30 - target_q30).abs() <= ORTHONORMAL_TOLERANCE_Q30
}

fn orthonormal(directions: &[AdapterDirection]) -> bool {
    let one = 1_i128 << UNIT_SCALE_BITS;
    directions.iter().enumerate().all(|(i, a)| {
        directions.iter().skip(i).enumerate().all(|(offset, b)| {
            let target = if offset == 0 { one } else { 0 };
            near(
                dot_q60(a.unit_q30.as_slice(), b.unit_q30.as_slice()),
                target,
            )
        })
    })
}

impl QueryAdapterBody {
    /// The validating check: a served version, `1..=8` directions of the
    /// declared width, bounded gains, orthonormal units.
    pub fn check(&self) -> Result<(), String> {
        if self.schema_version != QUERY_ADAPTER_SCHEMA_VERSION {
            return Err(format!(
                "adapter version {} is not served",
                self.schema_version
            ));
        }
        if self.space_digest.is_empty() || self.directions.is_empty() {
            return Err("an adapter names its space and carries a direction".to_string());
        }
        let width = self.dimensions as usize;
        let shaped = self.directions.iter().all(|d| {
            d.unit_q30.len() == width
                && d.gain_q16.unsigned_abs() <= MAX_ADAPTER_GAIN_Q16.unsigned_abs()
        });
        if !shaped {
            return Err("every direction has the declared width and |gain| <= 1/2".to_string());
        }
        if !orthonormal(self.directions.as_slice()) {
            return Err("adapter directions must be orthonormal".to_string());
        }
        Ok(())
    }
}

/// Fit an adapter from the judged retrieval outcomes of one space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterFitRequest {
    /// The graph whose stored vectors score the judged units. The caller's
    /// graph ACL and row-level security apply: an invisible unit is never read.
    pub graph: String,
    /// The embedding model identity the graph's vectors and the judged
    /// queries were produced in. When the graph's store declares a space it
    /// must be this one.
    pub space_digest: String,
    /// `None`: every retrieval question.
    #[serde(default)]
    pub question_id: Option<String>,
    pub window: RecordWindow,
    /// Directions to fit, `1..=8`.
    pub rank: u8,
    /// Largest `|g_i|` on `Q16`, at most one half.
    pub max_gain_q16: i32,
    /// Share of judged items held out for evaluation, per mille (`100..=500`).
    pub holdout_per_mille: u16,
    /// Fewest held-out items a passing receipt needs.
    pub min_eval_items: u32,
    /// What the adapter is fitted for.
    pub scope: AdapterScope,
    /// Which side of retrieval it transforms.
    pub side: AdapterSide,
    /// Which recorded outcomes train it; at least one.
    pub sources: Vec<OutcomeSource>,
}

/// The promotion receipt of one fitted adapter: a paired sign test of the
/// held-out reciprocal rank of the best cited unit, adapted vs base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterEvalReceipt {
    pub schema_version: u16,
    pub adapter_digest: String,
    /// The graph whose stored vectors it was fitted and evaluated on; the only
    /// graph it may be activated for.
    pub graph: String,
    pub space_digest: String,
    pub n_training: u64,
    pub n_eval: u64,
    pub wins: u64,
    pub losses: u64,
    pub ties: u64,
    /// Mean reciprocal rank on the held-out items, on `Q16`.
    pub base_mrr_q16: i64,
    pub adapted_mrr_q16: i64,
    /// Wilson 95% lower bound of `wins / (wins + losses)`, on `Q16`.
    pub win_rate_lower_q16: i64,
    /// Wilson 95% upper bound of `wins / (wins + losses)`, on `Q16`.
    pub win_rate_upper_q16: i64,
    /// What the adapter is fitted for.
    pub scope: AdapterScope,
    /// Which side of retrieval it transforms.
    pub side: AdapterSide,
    /// Each outcome source that trained it, with the items it contributed.
    pub sources: Vec<SourceCount>,
    /// Digest of the admitted training items, in order.
    pub training_digest: String,
    /// The refit counter within the scope; the first fit is revision 1.
    pub revision: u32,
    /// The adapter this one was refitted from, when the scope had one active.
    pub parent_digest: Option<String>,
    /// Passed: `n_eval >= min_eval_items` and the lower bound exceeds one half.
    pub passed: bool,
}

/// One outcome source of a fit and the items it contributed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceCount {
    pub source: OutcomeSource,
    pub count: u64,
}

/// Run the post-activation monitor of the scope's active adapter over the
/// outcomes judged in `window` (MEDULLA-DECISION-ENGINE-R174.7.2.1). A
/// verdict of rollback moves the scope's pointer back in the same write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterMonitorRequest {
    pub graph: String,
    pub scope: AdapterScope,
    pub window: RecordWindow,
}

/// The state machine of one monitor run: `watching` while the window holds
/// too few outcomes, `healthy`, or `rolled_back`; the only writer is the
/// monitor write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MonitorState {
    Watching,
    Healthy,
    RolledBack,
}

/// What the monitor saw and did: the window, the counts and the Wilson upper
/// bound that decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterMonitored {
    pub adapter_digest: String,
    pub scope: AdapterScope,
    pub state: MonitorState,
    pub window: RecordWindow,
    pub outcomes: u64,
    pub wins: u64,
    pub losses: u64,
    /// Wilson 95% upper bound of `wins / (wins + losses)`, on `Q16`.
    pub win_rate_upper_q16: i64,
    pub min_outcomes: u64,
}

/// A fitted adapter, its receipt and the digests they are pinned by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdapterFitted {
    pub adapter_digest: String,
    pub body: QueryAdapterBody,
    pub receipt_digest: String,
    pub receipt: AdapterEvalReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direction(unit: &[i32], gain_q16: i32) -> AdapterDirection {
        AdapterDirection {
            gain_q16,
            unit_q30: BoundedVec::new(unit.to_vec()).unwrap(),
        }
    }

    fn body(directions: Vec<AdapterDirection>) -> QueryAdapterBody {
        QueryAdapterBody {
            schema_version: QUERY_ADAPTER_SCHEMA_VERSION,
            space_digest: "sha256:space".to_string(),
            dimensions: 2,
            directions: BoundedVec::new(directions).unwrap(),
            training_digest: "sha256:t".to_string(),
            n_training: 1,
            revision: 1,
            parent_digest: None,
        }
    }

    fn fit_request_json(scope: &str, side: &str, source: &str) -> String {
        format!(
            r#"{{"graph":"g","space_digest":"s","window":{{"from_ms":0,"to_ms":1}},"rank":2,
            "max_gain_q16":100,"holdout_per_mille":200,"min_eval_items":5,
            "scope":{scope},"side":"{side}","sources":["{source}"]}}"#
        )
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.7.1
    #[test]
    fn scoped_fit_request_round_trips() {
        for scope in [
            r#"{"scope":"graph"}"#,
            r#"{"scope":"corpus","corpus_iri":"urn:c"}"#,
            r#"{"scope":"capability","capability_iri":"urn:k"}"#,
        ] {
            let json = fit_request_json(scope, "query", "retrieval_outcome");
            let request: AdapterFitRequest = serde_json::from_str(&json).unwrap();
            let again = serde_json::to_string(&request).unwrap();
            let back: AdapterFitRequest = serde_json::from_str(&again).unwrap();
            assert_eq!(back, request);
        }
        let index = fit_request_json(r#"{"scope":"graph"}"#, "index", "capability_outcome");
        let request: AdapterFitRequest = serde_json::from_str(&index).unwrap();
        assert_eq!(request.side, AdapterSide::Index);
        assert_eq!(request.sources, vec![OutcomeSource::CapabilityOutcome]);
        for bad in [
            fit_request_json(r#"{"scope":"planet"}"#, "query", "retrieval_outcome"),
            fit_request_json(r#"{"scope":"graph"}"#, "middle", "retrieval_outcome"),
            fit_request_json(r#"{"scope":"graph"}"#, "query", "rumour"),
        ] {
            assert!(serde_json::from_str::<AdapterFitRequest>(&bad).is_err());
        }
    }

    #[test]
    fn an_adapter_is_orthonormal_and_bounded() {
        let one = 1 << 30;
        assert!(
            body(vec![direction(&[one, 0], 100), direction(&[0, one], -100)])
                .check()
                .is_ok()
        );
        assert!(body(vec![direction(&[one, 0], 1 << 16)]).check().is_err());
        assert!(body(vec![direction(&[one, 0], 1), direction(&[one, 0], 1)])
            .check()
            .is_err());
        assert!(body(vec![direction(&[one / 2, 0], 1)]).check().is_err());
        assert!(body(vec![direction(&[one, 0, 0], 1)]).check().is_err());
    }
}
