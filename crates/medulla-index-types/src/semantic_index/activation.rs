//! The S6 payload of `SemanticIndexOp::CompleteGenerationStage`
//! (MEDULLA-UNIFIED-DATA-PLANE-R040.6, D110).
//!
//! The external stage worker finishes S6 by naming the generation's ANN image
//! as a content-addressed blob it uploaded, the exact S6 checkpoint (its
//! `checkpoint_digest` is the checkpoint id) and the ids each S4/S5 probe
//! query returned. The engine reads and verifies the blob itself, records the
//! probe measurements, judges the promotion verdict and only then activates.

use serde::{Deserialize, Serialize};

use super::identity::validate_text;
use super::{SemanticGenerationCheckpoint, SemanticIndexError};
use medulla_types::embedding_promotion::{PromotionModality, PromotionStage};

/// The most probe results one S6 completion may carry (two stages times the
/// closed modality set, with headroom).
pub const MAX_SEMANTIC_PROBE_RESULTS: usize = 16;

/// The most probe queries one probe result may answer.
pub const MAX_SEMANTIC_PROBE_QUERIES: usize = 1024;

/// The most ids one probe query may return.
pub const MAX_SEMANTIC_PROBE_IDS: usize = 1024;

/// The largest ANN image an S6 completion may name, in bytes.
pub const MAX_SEMANTIC_IMAGE_BYTES: u64 = 1 << 32;

/// A content-addressed blob the worker uploaded through the blob methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SemanticImageBlobRef {
    /// The blob manifest digest `BlobCommit` returned.
    pub manifest_digest: String,
    /// SHA-256 of the whole image body, 64 lowercase hex characters.
    pub sha256: String,
    /// The image body length in bytes.
    pub length: u64,
}

/// The ids each probe query of one stage and modality returned, in the probe
/// set's query order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SemanticProbeResult {
    pub stage: PromotionStage,
    pub modality: PromotionModality,
    pub retrieved: Vec<Vec<u64>>,
}

/// The S6 activation payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SemanticS6Activation {
    pub checkpoint: Box<SemanticGenerationCheckpoint>,
    pub image: SemanticImageBlobRef,
    pub probes: Vec<SemanticProbeResult>,
}

fn invalid(field: &str, reason: &str) -> SemanticIndexError {
    SemanticIndexError::InvalidField {
        field: field.to_string(),
        reason: reason.to_string(),
    }
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl SemanticImageBlobRef {
    pub fn validate(&self) -> Result<(), SemanticIndexError> {
        validate_text("activation.image.manifest_digest", &self.manifest_digest)?;
        if !is_lower_hex_digest(&self.sha256) {
            return Err(invalid(
                "activation.image.sha256",
                "must be 64 lowercase hex characters",
            ));
        }
        if self.length == 0 || self.length > MAX_SEMANTIC_IMAGE_BYTES {
            return Err(invalid("activation.image.length", "outside (0, 4 GiB]"));
        }
        Ok(())
    }
}

impl SemanticProbeResult {
    fn validate(&self) -> Result<(), SemanticIndexError> {
        let oversized = self.retrieved.len() > MAX_SEMANTIC_PROBE_QUERIES
            || self
                .retrieved
                .iter()
                .any(|ids| ids.len() > MAX_SEMANTIC_PROBE_IDS);
        if self.retrieved.is_empty() || oversized {
            return Err(invalid(
                "activation.probes.retrieved",
                "empty or over bound",
            ));
        }
        Ok(())
    }
}

impl SemanticS6Activation {
    /// Bounds only; the checkpoint and the image are verified against durable
    /// state by the store.
    pub fn validate(&self) -> Result<(), SemanticIndexError> {
        self.checkpoint.validate()?;
        self.image.validate()?;
        if self.probes.len() > MAX_SEMANTIC_PROBE_RESULTS {
            return Err(invalid("activation.probes", "over bound"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for probe in &self.probes {
            probe.validate()?;
            if !seen.insert((probe.stage, probe.modality)) {
                return Err(invalid("activation.probes", "duplicate stage and modality"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(sha256: &str, length: u64) -> SemanticImageBlobRef {
        SemanticImageBlobRef {
            manifest_digest: "m".repeat(64),
            sha256: sha256.to_string(),
            length,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.6
    #[test]
    fn image_blob_ref_bounds() {
        let cases = [
            (blob(&"a".repeat(64), 10), true),
            (blob(&"A".repeat(64), 10), false),
            (blob(&"a".repeat(63), 10), false),
            (blob(&"a".repeat(64), 0), false),
            (blob(&"a".repeat(64), MAX_SEMANTIC_IMAGE_BYTES + 1), false),
        ];
        for (reference, ok) in cases {
            assert_eq!(reference.validate().is_ok(), ok, "{reference:?}");
        }
    }

    #[test]
    fn probe_result_wire_names_stage_and_modality() {
        let probe = SemanticProbeResult {
            stage: PromotionStage::AnnIndex,
            modality: PromotionModality::Image,
            retrieved: vec![vec![1, 2]],
        };
        let json = serde_json::to_value(&probe).unwrap();
        assert_eq!(json["stage"], "S5");
        assert_eq!(json["modality"], "image");
        assert!(probe.validate().is_ok());
        let empty = SemanticProbeResult {
            retrieved: Vec::new(),
            ..probe
        };
        assert!(empty.validate().is_err());
    }
}
