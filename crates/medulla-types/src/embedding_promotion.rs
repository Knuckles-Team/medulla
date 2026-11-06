//! Per-modality promotion gate for a candidate embedding model
//! (MEDULLA-UNIFIED-DATA-PLANE-R040). A candidate model and version promote only
//! when every modality it serves holds its recall on that modality's probe
//! set. One regressed modality blocks promotion even when the mean improves.
//!
//! The measured recall at k per modality comes from `medulla_ann::recall_at_k`
//! over that modality's probe set; this module owns the typed checks, the
//! decision and the stored verdict shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The closed set of modalities a promotion check covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionModality {
    Text,
    Image,
    Audio,
}

/// The metric a modality check measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PromotionMetric {
    RecallAtK { k: u32 },
}

/// One modality's check: its probe set, the metric, the absolute floor and
/// the active version's baseline value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModalityCheck {
    pub modality: PromotionModality,
    pub probe_set_digest: String,
    pub metric: PromotionMetric,
    pub floor: f64,
    pub baseline: f64,
}

/// The candidate model and version, the binding generation it would make
/// `Live`, and the modalities it serves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionCandidate {
    pub binding_id: String,
    pub generation: u64,
    pub model: String,
    pub model_version: u64,
    pub modalities: Vec<PromotionModality>,
}

/// One modality's closed probe set (MEDULLA-UNIFIED-DATA-PLANE-R040.3): the
/// probe query vectors, the exact-neighbour ground-truth vector ids of each
/// query, the cut-off `k` and the recall floor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSet {
    pub k: u32,
    pub queries: Vec<Vec<f32>>,
    pub ground_truth: Vec<Vec<u64>>,
    pub recall_floor: f64,
}

impl ProbeSet {
    /// SHA-256 of the canonical probe-set block: 64 lowercase hex characters.
    pub fn digest(&self) -> String {
        let canonical = serde_json::to_vec(self).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(b"medulla/promotion-probe-set/v1\0");
        hasher.update(&canonical);
        hex::encode(hasher.finalize())
    }

    fn validate(&self, modality: PromotionModality) -> Result<(), PromotionConfigError> {
        let shaped = self.k > 0
            && !self.queries.is_empty()
            && self.queries.len() == self.ground_truth.len()
            && self.recall_floor.is_finite()
            && (0.0..=1.0).contains(&self.recall_floor)
            && self
                .queries
                .iter()
                .all(|query| !query.is_empty() && query.iter().all(|value| value.is_finite()));
        if shaped {
            Ok(())
        } else {
            Err(PromotionConfigError::ProbeSetMalformed(modality))
        }
    }
}

/// The engine-owned semantic model configuration (MEDULLA-UNIFIED-DATA-PLANE
/// -R040.3): the one config channel of the promotion gate. It names the model,
/// its operator-bumped `model_version`, the modalities the model serves, and
/// one digest-pinned probe set per served modality.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModelConfig {
    pub model_id: String,
    pub model_version: u64,
    pub modalities: Vec<PromotionModality>,
    pub probe_set: BTreeMap<PromotionModality, ProbeSet>,
    /// The pinned SHA-256 of each modality's probe-set block.
    pub probe_set_digests: BTreeMap<PromotionModality, String>,
}

/// Why a [`SemanticModelConfig`] cannot drive the promotion gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromotionConfigError {
    /// A served modality has no probe set.
    ProbeSetMissing(PromotionModality),
    /// A probe set's digest does not match its block.
    ProbeSetDigestMismatch(PromotionModality),
    /// A probe set is empty, ragged or has a floor outside `[0, 1]`.
    ProbeSetMalformed(PromotionModality),
    /// The config names no model or serves no modality.
    ModelMissing,
}

impl std::fmt::Display for PromotionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProbeSetMissing(modality) => write!(f, "{modality:?} has no probe set"),
            Self::ProbeSetDigestMismatch(modality) => {
                write!(f, "{modality:?} probe set digest does not match its block")
            }
            Self::ProbeSetMalformed(modality) => write!(f, "{modality:?} probe set is malformed"),
            Self::ModelMissing => f.write_str("semantic model config names no model or modality"),
        }
    }
}

impl SemanticModelConfig {
    /// Every served modality has a well-formed probe set whose pinned digest
    /// matches its block.
    pub fn validate(&self) -> Result<(), PromotionConfigError> {
        if self.model_id.trim().is_empty() || self.modalities.is_empty() {
            return Err(PromotionConfigError::ModelMissing);
        }
        for &modality in &self.modalities {
            let probe = self
                .probe_set
                .get(&modality)
                .ok_or(PromotionConfigError::ProbeSetMissing(modality))?;
            probe.validate(modality)?;
            if self.probe_set_digests.get(&modality) != Some(&probe.digest()) {
                return Err(PromotionConfigError::ProbeSetDigestMismatch(modality));
            }
        }
        Ok(())
    }
}

/// The S4 and S5 stages a probe measurement belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PromotionStage {
    #[serde(rename = "S4")]
    Vector,
    #[serde(rename = "S5")]
    AnnIndex,
}

impl PromotionStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vector => "S4",
            Self::AnnIndex => "S5",
        }
    }
}

impl PromotionModality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
        }
    }
}

/// The stored modality check of one model, binding and modality
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.3), built by the S4 and S5 stages from the
/// model's probe set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModalityCheckRecord {
    pub model_id: String,
    pub model_version: u64,
    pub binding_id: String,
    pub check: ModalityCheck,
    pub recorded_at_ms: u64,
}

/// The measured recall at k of one stage and modality of one generation
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.3), written by the stage itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageReceipt {
    pub binding_id: String,
    pub generation: u64,
    pub stage: PromotionStage,
    pub modality: PromotionModality,
    pub recall: f64,
    pub measured_at_ms: u64,
}

impl StageReceipt {
    /// `recall` is a finite fraction in `[0, 1]`.
    pub fn is_valid(&self) -> bool {
        self.recall.is_finite() && (0.0..=1.0).contains(&self.recall)
    }
}

/// The JSON record bytes every promotion-gate owner row stores.
pub fn encode_promotion_record<T: Serialize>(record: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(record).map_err(|error| error.to_string())
}

/// Decode one promotion-gate owner row.
pub fn decode_promotion_record<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| error.to_string())
}

/// Default absolute recall drop that blocks promotion. A named, replaceable
/// default; a measurement on this platform replaces it.
pub const DEFAULT_PROMOTION_TOLERANCE: f64 = 0.02;

/// The per-modality line of a verdict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModalityVerdict {
    pub modality: PromotionModality,
    pub probe_set_digest: Option<String>,
    pub metric: Option<PromotionMetric>,
    pub floor: Option<f64>,
    pub baseline: Option<f64>,
    pub measured: Option<f64>,
    pub passed: bool,
}

/// The stored promotion decision for one candidate, keyed by binding id and
/// generation and naming the model id and `u64` model version
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionVerdict {
    pub binding_id: String,
    pub generation: u64,
    pub model: String,
    pub model_version: u64,
    pub tolerance: f64,
    pub entries: Vec<ModalityVerdict>,
    pub promoted: bool,
}

impl PromotionVerdict {
    /// The refusal detail of a blocked verdict: each blocked modality with its
    /// baseline and measured value, or what it lacks.
    pub fn blocked_detail(&self) -> String {
        let lines: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| !entry.passed)
            .map(|entry| {
                let modality = entry.modality.as_str();
                match (entry.probe_set_digest.as_ref(), entry.measured) {
                    (None, _) => format!("{modality}: probe set missing"),
                    (Some(_), None) => format!("{modality}: measurement missing"),
                    (Some(_), Some(measured)) => format!(
                        "{modality}: baseline {} measured {measured} floor {}",
                        entry.baseline.unwrap_or_default(),
                        entry.floor.unwrap_or_default()
                    ),
                }
            })
            .collect();
        lines.join("; ")
    }

    /// The modalities that blocked promotion.
    pub fn blocked_modalities(&self) -> Vec<PromotionModality> {
        self.entries
            .iter()
            .filter(|entry| !entry.passed)
            .map(|entry| entry.modality)
            .collect()
    }
}

fn judge(
    modality: PromotionModality,
    check: Option<&ModalityCheck>,
    measured: Option<f64>,
    tolerance: f64,
) -> ModalityVerdict {
    let Some(check) = check.filter(|check| !check.probe_set_digest.trim().is_empty()) else {
        return ModalityVerdict {
            modality,
            probe_set_digest: None,
            metric: None,
            floor: None,
            baseline: None,
            measured,
            passed: false,
        };
    };
    let passed = measured.is_some_and(|value| {
        value.is_finite() && value >= check.floor && value + tolerance >= check.baseline
    });
    ModalityVerdict {
        modality,
        probe_set_digest: Some(check.probe_set_digest.clone()),
        metric: Some(check.metric),
        floor: Some(check.floor),
        baseline: Some(check.baseline),
        measured,
        passed,
    }
}

/// Decide whether `candidate` promotes. `measured` holds the candidate's
/// recall at k per modality on that modality's probe set. A modality the
/// candidate serves with no probe set, no measurement, a value under its
/// floor, or a drop from baseline beyond `tolerance` blocks promotion.
pub fn evaluate_promotion(
    candidate: &PromotionCandidate,
    checks: &[ModalityCheck],
    measured: &BTreeMap<PromotionModality, f64>,
    tolerance: f64,
) -> PromotionVerdict {
    let mut modalities = candidate.modalities.clone();
    modalities.sort();
    modalities.dedup();
    let entries: Vec<ModalityVerdict> = modalities
        .into_iter()
        .map(|modality| {
            let check = checks.iter().find(|check| check.modality == modality);
            judge(modality, check, measured.get(&modality).copied(), tolerance)
        })
        .collect();
    let promoted = !entries.is_empty() && entries.iter().all(|entry| entry.passed);
    PromotionVerdict {
        binding_id: candidate.binding_id.clone(),
        generation: candidate.generation,
        model: candidate.model.clone(),
        model_version: candidate.model_version,
        tolerance,
        entries,
        promoted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(modality: PromotionModality, baseline: f64) -> ModalityCheck {
        ModalityCheck {
            modality,
            probe_set_digest: format!("sha256:{modality:?}"),
            metric: PromotionMetric::RecallAtK { k: 10 },
            floor: 0.5,
            baseline,
        }
    }

    fn candidate(modalities: Vec<PromotionModality>) -> PromotionCandidate {
        PromotionCandidate {
            binding_id: "binding-a".to_string(),
            generation: 2,
            model: "clip-large".to_string(),
            model_version: 2,
            modalities,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.1
    #[test]
    fn checks_round_trip() {
        let original = check(PromotionModality::Image, 0.91);
        let json = serde_json::to_string(&original).unwrap();
        let back: ModalityCheck = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
        let unknown = json.replace("\"image\"", "\"video\"");
        assert!(serde_json::from_str::<ModalityCheck>(&unknown).is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.2
    #[test]
    fn one_regression_blocks_promotion() {
        use PromotionModality::{Audio, Image, Text};
        let checks = vec![check(Text, 0.80), check(Image, 0.90), check(Audio, 0.70)];
        // The mean improves (0.80 -> 0.85 avg), but image drops by 0.05.
        let measured = BTreeMap::from([(Text, 0.95), (Image, 0.85), (Audio, 0.75)]);
        let verdict = evaluate_promotion(
            &candidate(vec![Text, Image, Audio]),
            &checks,
            &measured,
            DEFAULT_PROMOTION_TOLERANCE,
        );
        assert!(!verdict.promoted);
        assert_eq!(verdict.blocked_modalities(), vec![Image]);

        let within = BTreeMap::from([(Text, 0.80), (Image, 0.89), (Audio, 0.70)]);
        let verdict = evaluate_promotion(
            &candidate(vec![Text, Image, Audio]),
            &checks,
            &within,
            DEFAULT_PROMOTION_TOLERANCE,
        );
        assert!(verdict.promoted, "a drop within the tolerance promotes");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.2
    #[test]
    fn modality_without_probe_set_blocks_promotion() {
        use PromotionModality::{Audio, Text};
        let measured = BTreeMap::from([(Text, 0.95), (Audio, 0.95)]);
        let verdict = evaluate_promotion(
            &candidate(vec![Text, Audio]),
            &[check(Text, 0.8)],
            &measured,
            DEFAULT_PROMOTION_TOLERANCE,
        );
        assert!(!verdict.promoted);
        assert_eq!(verdict.blocked_modalities(), vec![Audio]);
        let audio = verdict
            .entries
            .iter()
            .find(|e| e.modality == Audio)
            .unwrap();
        assert_eq!(audio.probe_set_digest, None);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.5
    #[test]
    fn verdict_is_stored_with_modality_entries() {
        use PromotionModality::{Image, Text};
        let probe = ProbeSet {
            k: 2,
            queries: vec![vec![1.0, 0.0]],
            ground_truth: vec![vec![1, 2]],
            recall_floor: 0.5,
        };
        let digest = probe.digest();
        assert_eq!(digest.len(), 64);
        assert!(digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        let checks = vec![
            ModalityCheck {
                modality: Text,
                probe_set_digest: digest.clone(),
                metric: PromotionMetric::RecallAtK { k: 2 },
                floor: 0.5,
                baseline: 0.8,
            },
            ModalityCheck {
                modality: Image,
                probe_set_digest: digest.clone(),
                metric: PromotionMetric::RecallAtK { k: 2 },
                floor: 0.5,
                baseline: 0.9,
            },
        ];
        let measured = BTreeMap::from([(Text, 0.85), (Image, 0.4)]);
        let verdict = evaluate_promotion(
            &candidate(vec![Text, Image]),
            &checks,
            &measured,
            DEFAULT_PROMOTION_TOLERANCE,
        );
        let bytes = encode_promotion_record(&verdict).unwrap();
        let stored: PromotionVerdict = decode_promotion_record(&bytes).unwrap();
        assert_eq!(stored, verdict);
        assert_eq!(
            (stored.binding_id.as_str(), stored.generation),
            ("binding-a", 2)
        );
        assert_eq!(
            (stored.model.as_str(), stored.model_version),
            ("clip-large", 2)
        );
        assert_eq!(stored.entries.len(), 2, "one entry per modality");
        let image = stored.entries.iter().find(|e| e.modality == Image).unwrap();
        assert_eq!(image.baseline, Some(0.9));
        assert_eq!(image.measured, Some(0.4));
        assert_eq!(image.probe_set_digest.as_deref(), Some(digest.as_str()));
        assert!(!image.passed);
        assert!(stored
            .blocked_detail()
            .contains("image: baseline 0.9 measured 0.4"));
    }
}
