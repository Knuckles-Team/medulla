//! The robust transfer estimate across model backbones (MEDULLA-DECISION-ENGINE-R059.1.1).
//!
//! An independent evaluation may name the backbone it was run under. The
//! aggregate read then reports, for one skill, the pooled success rate on each
//! declared backbone and the minimum of them: the robust rate.

use serde::{Deserialize, Serialize};

use super::numeric::QuantisedValue;
use super::record::outcome::RefusalCode;
use medulla_types::contract::BoundedVec;

/// Format identity of a [`TransferEstimate`].
pub const TRANSFER_SCHEMA_VERSION: u16 = 1;
/// Most backbones one transfer read declares.
pub const MAX_BACKBONES: usize = 16;

/// A declared backbone has no independent labelled row.
pub const BACKBONE_MISSING: RefusalCode = RefusalCode::new("BACKBONE_MISSING");
/// One `backbone_id` appears under two revisions.
pub const BACKBONE_DUPLICATE: RefusalCode = RefusalCode::new("BACKBONE_DUPLICATE");
/// Fewer declared backbones than `knob transfer.min_backbones`.
pub const TRANSFER_TOO_FEW_BACKBONES: RefusalCode = RefusalCode::new("TRANSFER_TOO_FEW_BACKBONES");
/// A backbone has fewer labels than `knob transfer.min_samples`.
pub const TRANSFER_SAMPLES_BELOW_MIN: RefusalCode = RefusalCode::new("TRANSFER_SAMPLES_BELOW_MIN");

/// The `transfer` family's own refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    BACKBONE_MISSING,
    BACKBONE_DUPLICATE,
    TRANSFER_TOO_FEW_BACKBONES,
    TRANSFER_SAMPLES_BELOW_MIN,
];

/// What an evaluation was run under: the model backbone, the skill it
/// exercised, the task distribution, the dataset and the metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BackboneBinding {
    pub backbone_id: String,
    pub backbone_revision: String,
    /// SHA-256 digest (64 lowercase hex characters) of the canonical skill identity.
    pub skill_key: String,
    pub task_distribution_id: String,
    pub task_distribution_revision: String,
    /// SHA-256 digest (64 lowercase hex characters) of the evaluation dataset.
    pub dataset_digest: String,
    pub metric_id: String,
    pub metric_revision: String,
}

/// Whether `value` is 64 lowercase hexadecimal characters.
pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BackboneBinding {
    /// The first malformed field, if any.
    pub fn validate(&self) -> Result<(), String> {
        let named = [
            ("backbone_id", &self.backbone_id),
            ("backbone_revision", &self.backbone_revision),
            ("task_distribution_id", &self.task_distribution_id),
            (
                "task_distribution_revision",
                &self.task_distribution_revision,
            ),
            ("metric_id", &self.metric_id),
            ("metric_revision", &self.metric_revision),
        ];
        if let Some((name, _)) = named.iter().find(|(_, v)| v.is_empty()) {
            return Err(format!("backbone binding field {name} is empty"));
        }
        for (name, digest) in [
            ("skill_key", &self.skill_key),
            ("dataset_digest", &self.dataset_digest),
        ] {
            if !is_digest(digest) {
                return Err(format!(
                    "backbone binding field {name} must be 64 lowercase hex characters"
                ));
            }
        }
        Ok(())
    }
}

/// Ask for the robust transfer estimate of one skill over declared backbones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TransferRequest {
    /// SHA-256 digest (64 lowercase hex characters) of the canonical skill identity.
    pub skill_key: String,
    pub declared_backbones: BoundedVec<String, MAX_BACKBONES>,
}

/// One backbone's row of a transfer estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BackboneRate {
    pub backbone_id: String,
    pub backbone_revision: String,
    pub trials: u64,
    pub successes: u64,
    /// The pooled (Beta-Binomial) success rate on this backbone.
    pub rate: QuantisedValue,
}

/// The robust transfer estimate: the minimum pooled rate over the declared backbones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TransferEstimate {
    pub schema_version: u16,
    pub skill_key: String,
    pub per_backbone: BoundedVec<BackboneRate, MAX_BACKBONES>,
    pub robust_rate: QuantisedValue,
    /// The backbone that sets the robust rate (the lowest id on a tie).
    pub min_backbone_id: String,
    /// The weakest class among the labels behind the estimate: an estimate is
    /// never stronger than its weakest label.
    pub evidence_class: super::record::EvidenceClass,
    /// `transfer.robust_rate`, `transfer.min_backbone` and one `knob:<name>=<value>`
    /// per knob the read used.
    #[serde(default)]
    pub premises: BoundedVec<super::record::PremiseRef, 8>,
}

/// One option that earned a skill candidate: repeated independent successes
/// under one question, from enough distinct records (MEDULLA-DECISION-ENGINE-R059.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillCandidateRow {
    pub question_id: String,
    pub option_id: String,
    /// Independent successful labels of the option under the question.
    pub successes: u64,
    /// Distinct records those successes come from.
    pub distinct_records: u64,
    /// `transfer.skill_candidate:<option_id>`, an Observation.
    pub premise: super::record::PremiseRef,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::QuantScaleTag;

    pub(crate) fn binding() -> BackboneBinding {
        BackboneBinding {
            backbone_id: "backbone-a".to_string(),
            backbone_revision: "1".to_string(),
            skill_key: "a".repeat(64),
            task_distribution_id: "tasks".to_string(),
            task_distribution_revision: "1".to_string(),
            dataset_digest: "b".repeat(64),
            metric_id: "success".to_string(),
            metric_revision: "1".to_string(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R059.1.1
    #[test]
    fn binding_and_estimate_round_trip() {
        let binding = binding();
        binding.validate().expect("a well-formed binding");
        let json = serde_json::to_string(&binding).unwrap();
        assert_eq!(
            serde_json::from_str::<BackboneBinding>(&json).unwrap(),
            binding
        );

        let mut short = binding.clone();
        short.skill_key = "ABC".to_string();
        assert!(short.validate().unwrap_err().contains("skill_key"));
        let mut upper = binding.clone();
        upper.dataset_digest = "A".repeat(64);
        assert!(upper.validate().unwrap_err().contains("dataset_digest"));
        let mut empty = binding.clone();
        empty.metric_id.clear();
        assert!(empty.validate().unwrap_err().contains("metric_id"));

        let request = TransferRequest {
            skill_key: binding.skill_key.clone(),
            declared_backbones: BoundedVec::new(vec!["backbone-a".into(), "backbone-b".into()])
                .unwrap(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<TransferRequest>(&json).unwrap(),
            request
        );

        let rate = |id: &str, value| BackboneRate {
            backbone_id: id.to_string(),
            backbone_revision: "1".to_string(),
            trials: 30,
            successes: 20,
            rate: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value,
            },
        };
        let estimate = TransferEstimate {
            schema_version: TRANSFER_SCHEMA_VERSION,
            skill_key: binding.skill_key,
            per_backbone: BoundedVec::new(vec![rate("backbone-a", 5), rate("backbone-b", 3)])
                .unwrap(),
            robust_rate: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 3,
            },
            min_backbone_id: "backbone-b".to_string(),
            evidence_class: crate::decision::EvidenceClass::Observation,
            premises: BoundedVec::default(),
        };
        let json = serde_json::to_string(&estimate).unwrap();
        assert_eq!(
            serde_json::from_str::<TransferEstimate>(&json).unwrap(),
            estimate
        );
        let unknown = json.replacen("{", "{\"extra\":1,", 1);
        assert!(serde_json::from_str::<TransferEstimate>(&unknown).is_err());
    }
}
