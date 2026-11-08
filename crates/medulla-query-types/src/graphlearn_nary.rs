//! MEDULLA-FEDERATED-QUERY-R086.100.1 — ordered n-ary inductive link prediction model.
//!
//! An [`NaryRequest`] scores ordered candidate tuples of 3..=`nary_max_arity` members
//! against a fitted link model; every result is an [`NaryHypothesis`], never a write.
//! The request carries no `writeback` field. Tuple order is significant: the request
//! digest covers each tuple in its given member order, so reordering a tuple changes
//! the digest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Registered `ServerErrorCode` refusal for a tuple outside the arity bounds (unary, binary or above the max).
pub const NARY_ARITY_INVALID: &str = "NARY_ARITY_INVALID";
/// Registered `ServerErrorCode` refusal for a candidate set above `graphlearn.nary_max_candidates`.
pub const NARY_CONSTRAINT_INVALID: &str = "NARY_CONSTRAINT_INVALID";

/// Registered `ServerErrorCode` refusal for a member that resolves to no node of the source.
pub const NARY_SOURCE_UNRESOLVED: &str = "NARY_SOURCE_UNRESOLVED";
/// Registered `ServerErrorCode` refusal for a member the caller's row-level-security snapshot hides.
pub const NARY_SOURCE_UNAUTHORIZED: &str = "NARY_SOURCE_UNAUTHORIZED";
/// Registered `ServerErrorCode` refusal for a pinned graph revision that no longer matches.
pub const NARY_REVISION_STALE: &str = "NARY_REVISION_STALE";

/// Smallest admitted tuple arity; unary and binary tuples refuse.
pub const NARY_MIN_ARITY: usize = 3;

/// How a hypothesis was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum NaryEvidenceClass {
    /// Every member was seen when the model was fitted.
    ObservedNeighbourhood,
    /// At least one member was unseen at fit and scored from its current neighbours.
    InductiveInference,
}

/// The registered bounds of the n-ary surface (`graphlearn.nary_max_arity`,
/// `graphlearn.nary_max_candidates`). Values outside their ranges refuse startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NaryBounds {
    /// `graphlearn.nary_max_arity`: default 6, range [3, 12], members.
    pub max_arity: usize,
    /// `graphlearn.nary_max_candidates`: default 1000, range [1, 100000], tuples.
    pub max_candidates: usize,
}

impl Default for NaryBounds {
    fn default() -> Self {
        Self {
            max_arity: 6,
            max_candidates: 1000,
        }
    }
}

impl NaryBounds {
    /// Range of `graphlearn.nary_max_arity`.
    pub const MAX_ARITY_RANGE: (usize, usize) = (3, 12);
    /// Range of `graphlearn.nary_max_candidates`.
    pub const MAX_CANDIDATES_RANGE: (usize, usize) = (1, 100_000);

    /// Startup bound check: a value out of its range is refused.
    pub fn validate(&self) -> Result<(), String> {
        let (arity_min, arity_max) = Self::MAX_ARITY_RANGE;
        if !(arity_min..=arity_max).contains(&self.max_arity) {
            return Err(format!(
                "graphlearn.nary_max_arity={} outside [{arity_min}, {arity_max}]",
                self.max_arity
            ));
        }
        let (cand_min, cand_max) = Self::MAX_CANDIDATES_RANGE;
        if !(cand_min..=cand_max).contains(&self.max_candidates) {
            return Err(format!(
                "graphlearn.nary_max_candidates={} outside [{cand_min}, {cand_max}]",
                self.max_candidates
            ));
        }
        Ok(())
    }
}

/// A typed refusal of an n-ary request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NaryRefusal {
    #[error("NARY_ARITY_INVALID: tuple {index} has {arity} members, admitted 3..={max}")]
    ArityInvalid {
        index: usize,
        arity: usize,
        max: usize,
    },
    #[error("NARY_CONSTRAINT_INVALID: {count} candidates exceed the bound {max}")]
    TooManyCandidates { count: usize, max: usize },
    #[error("NARY_SOURCE_UNRESOLVED: a member resolves to no node of the source")]
    SourceUnresolved,
    #[error("NARY_SOURCE_UNAUTHORIZED: a member is not visible to the caller")]
    SourceUnauthorized,
    #[error("NARY_REVISION_STALE: pinned graph revision {expected} but the graph is at {actual}")]
    RevisionStale { expected: u64, actual: u64 },
}

impl NaryRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ArityInvalid { .. } => NARY_ARITY_INVALID,
            Self::TooManyCandidates { .. } => NARY_CONSTRAINT_INVALID,
            Self::SourceUnresolved => NARY_SOURCE_UNRESOLVED,
            Self::SourceUnauthorized => NARY_SOURCE_UNAUTHORIZED,
            Self::RevisionStale { .. } => NARY_REVISION_STALE,
        }
    }
}

/// Ordered n-ary prediction request. No `writeback` field: results are hypotheses only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NaryRequest {
    /// The fitted model blob returned by `GraphLearnFit`.
    pub model: serde_json::Value,
    /// Ordered candidate tuples of node ids; member order is significant.
    pub candidates: Vec<Vec<String>>,
    /// Relation constraints the tuple's consecutive pairs must satisfy (relation labels).
    #[serde(default)]
    pub relation_constraints: Vec<String>,
    /// Number of hypotheses to return; 0 returns every admitted tuple.
    #[serde(default)]
    pub top_k: usize,
}

impl NaryRequest {
    /// Refuse unary/binary or over-long tuples and over-large candidate sets.
    pub fn validate(&self, bounds: &NaryBounds) -> Result<(), NaryRefusal> {
        if self.candidates.len() > bounds.max_candidates {
            return Err(NaryRefusal::TooManyCandidates {
                count: self.candidates.len(),
                max: bounds.max_candidates,
            });
        }
        for (index, tuple) in self.candidates.iter().enumerate() {
            if tuple.len() < NARY_MIN_ARITY || tuple.len() > bounds.max_arity {
                return Err(NaryRefusal::ArityInvalid {
                    index,
                    arity: tuple.len(),
                    max: bounds.max_arity,
                });
            }
        }
        Ok(())
    }

    /// Order-sensitive request digest (hex SHA-256) over model, candidates in member
    /// order, constraints and `top_k`. Each field is length-prefixed so no two
    /// distinct requests share an encoding.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        let mut put = |bytes: &[u8]| {
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        };
        put(self.model.to_string().as_bytes());
        put(&(self.candidates.len() as u64).to_le_bytes());
        for tuple in &self.candidates {
            put(&(tuple.len() as u64).to_le_bytes());
            for member in tuple {
                put(member.as_bytes());
            }
        }
        put(&(self.relation_constraints.len() as u64).to_le_bytes());
        for c in &self.relation_constraints {
            put(c.as_bytes());
        }
        put(&(self.top_k as u64).to_le_bytes());
        hex::encode(h.finalize())
    }
}

/// One scored ordered tuple. Read-only: it is never written back to the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NaryHypothesis {
    /// The tuple in request member order.
    pub tuple: Vec<String>,
    /// Score as Q16 fixed point (`round(p * 65536)`), so the wire stays integer.
    pub score_q16: u32,
    pub evidence_class: NaryEvidenceClass,
    /// Sources the members resolved from.
    pub source_ids: Vec<String>,
    pub graph_revision: u64,
    pub model_revision: String,
    pub policy_revision: u64,
}

impl NaryHypothesis {
    /// Convert a probability in [0, 1] to Q16.
    pub fn q16(p: f64) -> u32 {
        (p.clamp(0.0, 1.0) * 65536.0).round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(candidates: Vec<Vec<&str>>) -> NaryRequest {
        NaryRequest {
            model: serde_json::json!({"m": 1}),
            candidates: candidates
                .into_iter()
                .map(|t| t.into_iter().map(String::from).collect())
                .collect(),
            relation_constraints: vec![],
            top_k: 0,
        }
    }

    #[test]
    fn order_changes_digest_and_arity_is_bounded() {
        // spec: MEDULLA-FEDERATED-QUERY-R086.100.1
        let bounds = NaryBounds::default();
        assert!(bounds.validate().is_ok());
        let a = req(vec![vec!["x", "y", "z"]]);
        let b = req(vec![vec!["y", "x", "z"]]);
        assert_ne!(
            a.digest(),
            b.digest(),
            "member order must change the digest"
        );
        assert_eq!(a.digest(), a.clone().digest());
        assert!(a.validate(&bounds).is_ok());

        for bad in [vec!["x"], vec!["x", "y"], vec!["a"; 7]] {
            let err = req(vec![bad]).validate(&bounds).unwrap_err();
            assert_eq!(err.code(), NARY_ARITY_INVALID);
        }
        assert!(req(vec![vec!["a"; 6]]).validate(&bounds).is_ok());

        let many = req(vec![vec!["a", "b", "c"]; 1001]);
        assert_eq!(
            many.validate(&bounds).unwrap_err().code(),
            NARY_CONSTRAINT_INVALID
        );

        for out in [
            NaryBounds {
                max_arity: 2,
                ..bounds
            },
            NaryBounds {
                max_arity: 13,
                ..bounds
            },
            NaryBounds {
                max_candidates: 0,
                ..bounds
            },
            NaryBounds {
                max_candidates: 100_001,
                ..bounds
            },
        ] {
            assert!(out.validate().is_err());
        }

        // No writeback field is accepted on the wire.
        let raw = serde_json::json!({"model": {}, "candidates": [], "writeback": true});
        assert!(serde_json::from_value::<NaryRequest>(raw).is_err());
    }

    #[test]
    fn nary_refusals_are_registered_server_error_codes() {
        use medulla_types::contract::ServerErrorCode;
        for code in [
            NARY_ARITY_INVALID,
            NARY_CONSTRAINT_INVALID,
            NARY_SOURCE_UNRESOLVED,
            NARY_SOURCE_UNAUTHORIZED,
            NARY_REVISION_STALE,
        ] {
            assert!(
                ServerErrorCode::ALL
                    .iter()
                    .any(|item| item.as_str() == code),
                "{code} is not a ServerErrorCode"
            );
        }
    }
}
