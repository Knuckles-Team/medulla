//! MEDULLA-DECISION-ENGINE-R706.1: entity-resolution proposal vocabulary --
//! the resolution tiers, tenant-scoped embeddings, the proposals a family
//! returns, the review outcome of one proposal and the per-tier evaluation the
//! drift monitor reads.
//!
//! A proposal is only a proposal: a probabilistic completion sits beside the
//! exact answer and never replaces it. Every embedding and proposal carries its
//! tenant so a candidate never crosses tenants.

use serde::{Deserialize, Serialize};

/// Where a candidate pair came from, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResolutionTier {
    Exact,
    Embedding,
    Probabilistic,
}

/// The review verdict on one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReviewOutcome {
    Accepted,
    Rejected,
}

/// A node's embedding, held under its own tenant. `content_hash` is the
/// SHA-256 of the embedded text: 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TenantScopedEmbedding {
    pub tenant: String,
    pub node: String,
    pub model: String,
    pub content_hash: String,
    pub vector: Vec<f32>,
}

/// A proposed match between two nodes of one tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EntityResolutionProposal {
    pub proposal_id: String,
    pub tenant: String,
    pub left: String,
    pub right: String,
    /// Similarity in parts per million, `0..=1_000_000`.
    pub score_ppm: u32,
    pub tier: ResolutionTier,
    pub model: String,
    pub content_hash: String,
}

/// How one tier's proposals were reviewed. `accept_rate_ppm` is
/// `accepted / (accepted + rejected)` in parts per million, 0 with no reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TierEvaluation {
    pub tier: ResolutionTier,
    pub accepted: u32,
    pub rejected: u32,
    pub accept_rate_ppm: u32,
}

impl TierEvaluation {
    /// The evaluation of `tier` after `accepted` and `rejected` reviews.
    pub fn new(tier: ResolutionTier, accepted: u32, rejected: u32) -> Self {
        let total = u64::from(accepted) + u64::from(rejected);
        let accept_rate_ppm = if total == 0 {
            0
        } else {
            (u64::from(accepted) * 1_000_000 / total) as u32
        };
        Self {
            tier,
            accepted,
            rejected,
            accept_rate_ppm,
        }
    }
}

/// Whether `hash` is a SHA-256 content hash: 64 lowercase hexadecimal characters.
pub fn is_content_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl TenantScopedEmbedding {
    /// Refuse an embedding whose hash is not a SHA-256 hex digest.
    pub fn validate(&self) -> Result<(), String> {
        if is_content_hash(&self.content_hash) {
            Ok(())
        } else {
            Err("content_hash must be 64 lowercase hex characters".into())
        }
    }
}

impl EntityResolutionProposal {
    /// Refuse a proposal whose score is out of range or whose hash is not a
    /// SHA-256 hex digest.
    pub fn validate(&self) -> Result<(), String> {
        if self.score_ppm > 1_000_000 {
            return Err("score_ppm must be at most 1000000".into());
        }
        if !is_content_hash(&self.content_hash) {
            return Err("content_hash must be 64 lowercase hex characters".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash() -> String {
        "ab".repeat(32)
    }

    // spec: MEDULLA-DECISION-ENGINE-R706.1
    #[test]
    fn er_types_round_trip() {
        let embedding = TenantScopedEmbedding {
            tenant: "t1".into(),
            node: "n1".into(),
            model: "m".into(),
            content_hash: hash(),
            vector: vec![0.5, -0.25],
        };
        let json = serde_json::to_string(&embedding).unwrap();
        assert_eq!(
            serde_json::from_str::<TenantScopedEmbedding>(&json).unwrap(),
            embedding
        );
        assert!(embedding.validate().is_ok());

        let proposal = EntityResolutionProposal {
            proposal_id: "p1".into(),
            tenant: "t1".into(),
            left: "n1".into(),
            right: "n2".into(),
            score_ppm: 910_000,
            tier: ResolutionTier::Embedding,
            model: "m".into(),
            content_hash: hash(),
        };
        let json = serde_json::to_string(&proposal).unwrap();
        assert!(json.contains(r#""tier":"embedding""#), "{json}");
        assert_eq!(
            serde_json::from_str::<EntityResolutionProposal>(&json).unwrap(),
            proposal
        );
        assert!(proposal.validate().is_ok());

        for tier in [
            ResolutionTier::Exact,
            ResolutionTier::Embedding,
            ResolutionTier::Probabilistic,
        ] {
            let json = serde_json::to_string(&tier).unwrap();
            assert_eq!(serde_json::from_str::<ResolutionTier>(&json).unwrap(), tier);
        }
        for outcome in [ReviewOutcome::Accepted, ReviewOutcome::Rejected] {
            let json = serde_json::to_string(&outcome).unwrap();
            assert_eq!(
                serde_json::from_str::<ReviewOutcome>(&json).unwrap(),
                outcome
            );
        }
        let evaluation = TierEvaluation::new(ResolutionTier::Probabilistic, 3, 1);
        assert_eq!(evaluation.accept_rate_ppm, 750_000);
        assert_eq!(
            TierEvaluation::new(ResolutionTier::Exact, 0, 0).accept_rate_ppm,
            0
        );
        let json = serde_json::to_string(&evaluation).unwrap();
        assert_eq!(
            serde_json::from_str::<TierEvaluation>(&json).unwrap(),
            evaluation
        );

        // Unknown tiers, unknown fields, a bad hash and an out-of-range score refuse.
        assert!(serde_json::from_str::<ResolutionTier>(r#""fuzzy""#).is_err());
        assert!(serde_json::from_str::<ReviewOutcome>(r#""maybe""#).is_err());
        assert!(serde_json::from_str::<TierEvaluation>(
            r#"{"tier":"exact","accepted":1,"rejected":0,"accept_rate_ppm":1000000,"x":1}"#
        )
        .is_err());
        let bad_hash = EntityResolutionProposal {
            content_hash: "ABC".into(),
            ..proposal.clone()
        };
        assert!(bad_hash.validate().is_err());
        let upper = EntityResolutionProposal {
            content_hash: "AB".repeat(32),
            ..proposal.clone()
        };
        assert!(upper.validate().is_err());
        let high = EntityResolutionProposal {
            score_ppm: 1_000_001,
            ..proposal
        };
        assert!(high.validate().is_err());
    }
}
