//! The promotion gate of S6 activation (MEDULLA-UNIFIED-DATA-PLANE-R040.3-.5).
//!
//! A generation whose model id, model digest or operator-bumped
//! `model_version` differs from the binding's current `Live` generation goes
//! `Live` only on a passing [`PromotionVerdict`]. The gate reads the stored
//! [`ModalityCheckRecord`] of each modality the model serves and the measured
//! recall of that modality's [`StageReceipt`], and judges them with
//! [`evaluate_promotion`]. A pass writes the verdict and the per-binding
//! embedding-version tags in the activation's own semantic-store commit; a
//! block writes the verdict alone, in its own commit, and refuses the
//! activation with [`PROMOTION_BLOCKED`] so the prior generation stays `Live`.
//!
//! The S4 and S5 stages write the records the gate reads through
//! [`SemanticCodeStore::record_probe_measurement`]: recall at k over the
//! model config's probe set, measured with `medulla_ann::recall_at_k`, and the
//! prior `Live` generation's recall for the same stage and modality as the
//! baseline.

use std::collections::BTreeMap;

use super::batch::{semantic_digest, MetadataMutation};
use super::persist::replace_bytes;
use super::{kernel_error, refused, SemanticCodeError, SemanticCodeStore};
use medulla_ann::{recall_at_k, EmbeddingVersion, EmbeddingVersionStore, PromotionApproval};
use medulla_index_types::semantic_index::SemanticBinding;
use medulla_semantic_index::semantic::SemanticGenerationImage;
use medulla_storage::{
    ScopedRead, SemanticIndexOwner, SEMANTIC_EMBEDDING_VERSION_TAGS, SEMANTIC_MODALITY_CHECKS,
    SEMANTIC_PROMOTION_VERDICTS, SEMANTIC_STAGE_RECEIPTS,
};
use medulla_transaction::AdmittedOwnerWrite;
use medulla_types::embedding_promotion::{
    decode_promotion_record, encode_promotion_record, evaluate_promotion, ModalityCheck,
    ModalityCheckRecord, PromotionCandidate, PromotionMetric, PromotionModality, PromotionStage,
    PromotionVerdict, SemanticModelConfig, StageReceipt, DEFAULT_PROMOTION_TOLERANCE,
};
use redb::ReadableTable;

/// The refusal code of a blocked promotion: the registered
/// `ServerErrorCode::PromotionBlocked`.
pub const PROMOTION_BLOCKED: &str = "PROMOTION_BLOCKED";

/// A failed verdict: the activation is refused and the verdict is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionBlocked {
    pub verdict: PromotionVerdict,
}

/// A [`PromotionVerdict`] as the approval `EmbeddingVersionStore::tag_batch`
/// requires.
struct VerdictApproval<'a>(&'a PromotionVerdict);

impl PromotionApproval for VerdictApproval<'_> {
    fn promoted(&self) -> bool {
        self.0.promoted
    }
    fn model_id(&self) -> &str {
        &self.0.model
    }
    fn model_version(&self) -> u64 {
        self.0.model_version
    }
}

fn blocked(detail: &str) -> SemanticCodeError {
    SemanticCodeError::Refused(format!("{PROMOTION_BLOCKED}: {detail}"))
}

fn record_error(error: String) -> SemanticCodeError {
    SemanticCodeError::Corrupt(format!("promotion record: {error}"))
}

/// A binding with no model config serves text only.
fn served_modalities(config: Option<&SemanticModelConfig>) -> Vec<PromotionModality> {
    config.map_or_else(
        || vec![PromotionModality::Text],
        |config| config.modalities.clone(),
    )
}

/// The probe-set digest the config pins for `modality`, when its block
/// matches the pin.
fn pinned_digest(config: &SemanticModelConfig, modality: PromotionModality) -> Option<String> {
    let probe = config.probe_set.get(&modality)?;
    let digest = probe.digest();
    (config.probe_set_digests.get(&modality) == Some(&digest)).then_some(digest)
}

impl SemanticCodeStore {
    /// Install the engine-owned model config this binding's gate reads. A
    /// config without a probe set for a served modality, or whose probe-set
    /// digest does not match its block, is refused with `PROMOTION_BLOCKED`.
    pub fn install_model_config(
        &self,
        config: SemanticModelConfig,
    ) -> Result<(), SemanticCodeError> {
        config
            .validate()
            .map_err(|error| blocked(&error.to_string()))?;
        *self
            .model_config
            .write()
            .map_err(|_| refused("semantic model config lock poisoned"))? = Some(config);
        Ok(())
    }

    fn model_config(&self) -> Result<Option<SemanticModelConfig>, SemanticCodeError> {
        self.model_config
            .read()
            .map(|config| config.clone())
            .map_err(|_| refused("semantic model config lock poisoned"))
    }

    /// S4/S5: measure recall at k of one modality over the model config's
    /// probe set and store the stage's [`StageReceipt`] and the model's
    /// [`ModalityCheckRecord`] in one semantic-store commit. `retrieved` holds
    /// the ids the stage returned for each probe query, in query order.
    pub fn record_probe_measurement(
        &self,
        stage: PromotionStage,
        generation: u64,
        modality: PromotionModality,
        retrieved: &[Vec<u64>],
        now_ms: u64,
    ) -> Result<StageReceipt, SemanticCodeError> {
        let config = self
            .model_config()?
            .ok_or_else(|| refused("probe measurement requires an installed model config"))?;
        let probe_set_digest = pinned_digest(&config, modality)
            .ok_or_else(|| refused("probe measurement requires the modality's pinned probe set"))?;
        let probe = &config.probe_set[&modality];
        if retrieved.len() != probe.queries.len() {
            return Err(refused(
                "probe measurement requires one result list per probe query",
            ));
        }
        let k = probe.k as usize;
        let recall = retrieved
            .iter()
            .zip(&probe.ground_truth)
            .map(|(found, truth)| recall_at_k(found, truth, k))
            .sum::<f64>()
            / retrieved.len() as f64;
        let read = self.door.serving_read()?;
        let baseline = match self.live_generation_in(&read)? {
            Some(live) if live != generation => self
                .read_stage_receipt_in(&read, live, stage, modality)?
                .map(|receipt| receipt.recall),
            _ => None,
        };
        drop(read);
        let receipt = StageReceipt {
            binding_id: self.binding.clone(),
            generation,
            stage,
            modality,
            recall,
            measured_at_ms: now_ms,
        };
        if !receipt.is_valid() {
            return Err(refused("probe measurement recall is outside [0, 1]"));
        }
        // A binding with no prior `Live` generation is judged against the
        // floor alone: its baseline is zero.
        let record = ModalityCheckRecord {
            model_id: config.model_id.clone(),
            model_version: config.model_version,
            binding_id: self.binding.clone(),
            check: ModalityCheck {
                modality,
                probe_set_digest,
                metric: PromotionMetric::RecallAtK { k: probe.k },
                floor: probe.recall_floor,
                baseline: baseline.unwrap_or(0.0),
            },
            recorded_at_ms: now_ms,
        };
        let receipt_bytes = encode_promotion_record(&receipt).map_err(record_error)?;
        let record_bytes = encode_promotion_record(&record).map_err(record_error)?;
        let mut digest_input = receipt_bytes.clone();
        digest_input.extend_from_slice(&record_bytes);
        let digest = semantic_digest(&digest_input);
        let batch_id = format!(
            "semantic-index:probe:{}:{generation}:{digest}",
            stage.as_str()
        );
        let subject = format!("generation:{generation}");
        let (tenant, binding) = (self.tenant.as_str(), self.binding.as_str());
        self.commit_maintenance(
            MetadataMutation {
                batch_id: &batch_id,
                event_type: "semantic_probe_measured",
                subject: &subject,
                mutation_digest: digest,
            },
            Vec::new(),
            now_ms,
            None,
            |_, rows| {
                let mut receipts = rows
                    .open_table(SEMANTIC_STAGE_RECEIPTS)
                    .map_err(kernel_error)?;
                replace_bytes(
                    &mut receipts,
                    (
                        tenant,
                        binding,
                        generation,
                        stage.as_str(),
                        modality.as_str(),
                    ),
                    &receipt_bytes,
                )?;
                drop(receipts);
                let mut checks = rows
                    .open_table(SEMANTIC_MODALITY_CHECKS)
                    .map_err(kernel_error)?;
                replace_bytes(
                    &mut checks,
                    (
                        tenant,
                        binding,
                        record.model_version,
                        record.model_id.as_str(),
                        modality.as_str(),
                    ),
                    &record_bytes,
                )
            },
        )?;
        Ok(receipt)
    }

    /// The gate of one S6 activation. `Ok(None)`: the generation keeps the
    /// live model and version, so no gate applies. `Ok(Some(verdict))`: a
    /// passing verdict the activation commit stores. `Err(blocked)`: the
    /// activation is refused.
    pub(super) fn promotion_gate(
        &self,
        read: &ScopedRead<'_, SemanticIndexOwner>,
        binding: &SemanticBinding,
        generation: u64,
    ) -> Result<Result<Option<PromotionVerdict>, PromotionBlocked>, SemanticCodeError> {
        let Some(live) = self.live_generation_in(read)? else {
            return Ok(Ok(None));
        };
        if live == generation {
            return Ok(Ok(None));
        }
        let config = self.model_config()?;
        let model_version = config.as_ref().map_or(0, |config| config.model_version);
        let prior = self
            .read_binding_generation_in(read, live)?
            .ok_or_else(|| refused("promotion gate requires the live generation's binding"))?;
        let prior_version = self
            .read_verdict_in(read, live)?
            .map(|verdict| verdict.model_version);
        let model_changed = (prior.model_id.as_str(), prior.model_digest.as_str())
            != (binding.model_id.as_str(), binding.model_digest.as_str())
            || prior_version.is_some_and(|version| version != model_version);
        if !model_changed {
            return Ok(Ok(None));
        }
        let modalities = served_modalities(config.as_ref());
        let mut checks = Vec::new();
        let mut measured = BTreeMap::new();
        for &modality in &modalities {
            let pinned = config
                .as_ref()
                .and_then(|config| pinned_digest(config, modality));
            let stored = self.read_check_in(read, model_version, &binding.model_id, modality)?;
            if let (Some(pinned), Some(record)) = (pinned, stored) {
                if record.check.probe_set_digest == pinned {
                    checks.push(record.check);
                }
            }
            let receipt = match self.read_stage_receipt_in(
                read,
                generation,
                PromotionStage::AnnIndex,
                modality,
            )? {
                Some(receipt) => Some(receipt),
                None => {
                    self.read_stage_receipt_in(read, generation, PromotionStage::Vector, modality)?
                }
            };
            if let Some(receipt) = receipt {
                measured.insert(modality, receipt.recall);
            }
        }
        let candidate = PromotionCandidate {
            binding_id: self.binding.clone(),
            generation,
            model: binding.model_id.clone(),
            model_version,
            modalities,
        };
        let verdict =
            evaluate_promotion(&candidate, &checks, &measured, DEFAULT_PROMOTION_TOLERANCE);
        Ok(if verdict.promoted {
            Ok(Some(verdict))
        } else {
            Err(PromotionBlocked { verdict })
        })
    }

    /// R040.4: store a blocked verdict in its own semantic-store commit (the
    /// activation commit does not happen) and return the refusal.
    pub(super) fn refuse_blocked_promotion(
        &self,
        blocked_promotion: PromotionBlocked,
        now_ms: u64,
    ) -> SemanticCodeError {
        let verdict = blocked_promotion.verdict;
        let detail = verdict.blocked_detail();
        let stored = encode_promotion_record(&verdict)
            .map_err(record_error)
            .and_then(|bytes| {
                let digest = semantic_digest(&bytes);
                let batch_id = format!(
                    "semantic-index:promotion-verdict:{}:{digest}",
                    verdict.generation
                );
                let subject = format!("generation:{}", verdict.generation);
                self.commit_maintenance(
                    MetadataMutation {
                        batch_id: &batch_id,
                        event_type: "semantic_promotion_blocked",
                        subject: &subject,
                        mutation_digest: digest,
                    },
                    Vec::new(),
                    now_ms,
                    None,
                    |_, rows| self.write_verdict(rows, &verdict, &bytes),
                )
            });
        match stored {
            Ok(_) => blocked(&detail),
            Err(error) => error,
        }
    }

    /// R040.3/.5: inside the activation write, store the passing verdict and
    /// tag every vector of the published image with the promoted version.
    pub(super) fn record_promotion_in_write(
        &self,
        rows: &AdmittedOwnerWrite<'_, SemanticIndexOwner>,
        verdict: &PromotionVerdict,
        image: &SemanticGenerationImage,
    ) -> Result<(), SemanticCodeError> {
        let bytes = encode_promotion_record(verdict).map_err(record_error)?;
        self.write_verdict(rows, verdict, &bytes)?;
        let ids = image
            .index
            .vector_ids()
            .map_err(|error| SemanticCodeError::Corrupt(error.to_string()))?;
        let (tenant, binding) = (self.tenant.as_str(), self.binding.as_str());
        let mut tags = rows
            .open_table(SEMANTIC_EMBEDDING_VERSION_TAGS)
            .map_err(kernel_error)?;
        let mut existing = Vec::new();
        for &id in &ids {
            let found = tags.get((tenant, binding, id)).map_err(kernel_error)?;
            if let Some(raw) = found {
                let version: EmbeddingVersion =
                    decode_promotion_record(raw.value()).map_err(record_error)?;
                existing.push((id, version));
            }
        }
        let mut store = EmbeddingVersionStore::restore(existing);
        let version = EmbeddingVersion::new(verdict.model.clone(), verdict.model_version);
        store
            .tag_batch(&ids, &version, &VerdictApproval(verdict))
            .map_err(|error| refused(&error.to_string()))?;
        for &id in &ids {
            let tagged = store
                .version_of(id)
                .ok_or_else(|| refused("embedding version tag was not applied"))?;
            let bytes = encode_promotion_record(tagged).map_err(record_error)?;
            replace_bytes(&mut tags, (tenant, binding, id), &bytes)?;
        }
        Ok(())
    }

    fn write_verdict(
        &self,
        rows: &AdmittedOwnerWrite<'_, SemanticIndexOwner>,
        verdict: &PromotionVerdict,
        bytes: &[u8],
    ) -> Result<(), SemanticCodeError> {
        let mut verdicts = rows
            .open_table(SEMANTIC_PROMOTION_VERDICTS)
            .map_err(kernel_error)?;
        replace_bytes(
            &mut verdicts,
            (
                self.tenant.as_str(),
                self.binding.as_str(),
                verdict.generation,
            ),
            bytes,
        )
    }

    /// The stored verdict of one generation, read back by stage status and
    /// by the catalog.
    pub fn promotion_verdict(
        &self,
        generation: u64,
    ) -> Result<Option<PromotionVerdict>, SemanticCodeError> {
        let read = self.door.serving_read()?;
        self.read_verdict_in(&read, generation)
    }

    fn read_verdict_in(
        &self,
        read: &ScopedRead<'_, SemanticIndexOwner>,
        generation: u64,
    ) -> Result<Option<PromotionVerdict>, SemanticCodeError> {
        let table = read
            .open_owner_table(SEMANTIC_PROMOTION_VERDICTS)
            .map_err(kernel_error)?;
        let found = table
            .get((self.tenant.as_str(), self.binding.as_str(), generation))
            .map_err(kernel_error)?;
        found
            .map(|raw| decode_promotion_record(raw.value()).map_err(record_error))
            .transpose()
    }

    fn read_check_in(
        &self,
        read: &ScopedRead<'_, SemanticIndexOwner>,
        model_version: u64,
        model_id: &str,
        modality: PromotionModality,
    ) -> Result<Option<ModalityCheckRecord>, SemanticCodeError> {
        let table = read
            .open_owner_table(SEMANTIC_MODALITY_CHECKS)
            .map_err(kernel_error)?;
        let found = table
            .get((
                self.tenant.as_str(),
                self.binding.as_str(),
                model_version,
                model_id,
                modality.as_str(),
            ))
            .map_err(kernel_error)?;
        found
            .map(|raw| decode_promotion_record(raw.value()).map_err(record_error))
            .transpose()
    }

    fn read_stage_receipt_in(
        &self,
        read: &ScopedRead<'_, SemanticIndexOwner>,
        generation: u64,
        stage: PromotionStage,
        modality: PromotionModality,
    ) -> Result<Option<StageReceipt>, SemanticCodeError> {
        let table = read
            .open_owner_table(SEMANTIC_STAGE_RECEIPTS)
            .map_err(kernel_error)?;
        let found = table
            .get((
                self.tenant.as_str(),
                self.binding.as_str(),
                generation,
                stage.as_str(),
                modality.as_str(),
            ))
            .map_err(kernel_error)?;
        found
            .map(|raw| decode_promotion_record(raw.value()).map_err(record_error))
            .transpose()
    }

    /// The embedding-version tag of one stored vector.
    pub fn embedding_version_of(
        &self,
        vector_id: u64,
    ) -> Result<Option<EmbeddingVersion>, SemanticCodeError> {
        let read = self.door.serving_read()?;
        let table = read
            .open_owner_table(SEMANTIC_EMBEDDING_VERSION_TAGS)
            .map_err(kernel_error)?;
        let found = table
            .get((self.tenant.as_str(), self.binding.as_str(), vector_id))
            .map_err(kernel_error)?;
        found
            .map(|raw| decode_promotion_record(raw.value()).map_err(record_error))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_ann_codes::tests::{seeded_s6_publication, S6Fixture};
    use medulla_mutation_types::mutation_scope::MutationOutboxLease;
    use medulla_transaction::OutboxClaimBudget;
    use medulla_types::embedding_promotion::ProbeSet;

    fn claim(fixture: &S6Fixture) -> MutationOutboxLease {
        let mut budget = OutboxClaimBudget::new(1, 5_000, 11).unwrap();
        fixture
            .codes
            .claim_stage_leases(fixture.consumer, &mut budget)
            .unwrap()
            .claims
            .into_iter()
            .next()
            .expect("the seeded S6 intent is claimable")
    }

    fn finalize(
        fixture: &S6Fixture,
        lease: &MutationOutboxLease,
    ) -> Result<super::super::SemanticMutationReceipt, SemanticCodeError> {
        fixture.codes.finalize_generation(
            lease,
            &fixture.s6_transition,
            &fixture.s6_checkpoint,
            &fixture.activation_artifact,
            &fixture.image,
            13,
        )
    }

    /// A text model at `model_version` whose probe set has one query with
    /// ground truth `[1, 2]` at k = 2 and a 0.5 recall floor.
    fn config(fixture: &S6Fixture, model_version: u64) -> SemanticModelConfig {
        let probe = ProbeSet {
            k: 2,
            queries: vec![vec![1.0, 0.0]],
            ground_truth: vec![vec![1, 2]],
            recall_floor: 0.5,
        };
        let digest = probe.digest();
        SemanticModelConfig {
            model_id: fixture.generation_two.model_id.clone(),
            model_version,
            modalities: vec![PromotionModality::Text],
            probe_set: BTreeMap::from([(PromotionModality::Text, probe)]),
            probe_set_digests: BTreeMap::from([(PromotionModality::Text, digest)]),
        }
    }

    /// Generation one is live under model version 1, measured at full recall.
    fn seed_live_version_one(fixture: &S6Fixture) {
        let codes = &fixture.codes;
        codes.install_model_config(config(fixture, 1)).unwrap();
        let baseline = codes
            .record_probe_measurement(
                PromotionStage::AnnIndex,
                1,
                PromotionModality::Text,
                &[vec![1, 2]],
                12,
            )
            .unwrap();
        assert_eq!(baseline.recall, 1.0);
        let prior = PromotionVerdict {
            binding_id: codes.binding.clone(),
            generation: 1,
            model: fixture.generation_two.model_id.clone(),
            model_version: 1,
            tolerance: DEFAULT_PROMOTION_TOLERANCE,
            entries: Vec::new(),
            promoted: true,
        };
        let bytes = encode_promotion_record(&prior).unwrap();
        let digest = semantic_digest(&bytes);
        codes
            .commit_maintenance(
                MetadataMutation {
                    batch_id: "semantic-index:promotion-seed:1",
                    event_type: "semantic_promotion_seed",
                    subject: "generation:1",
                    mutation_digest: digest,
                },
                Vec::new(),
                12,
                None,
                |_, rows| codes.write_verdict(rows, &prior, &bytes),
            )
            .unwrap();
        codes.install_model_config(config(fixture, 2)).unwrap();
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.3
    #[test]
    fn activation_runs_gate() {
        let fixture = seeded_s6_publication("promotion-gate-pass");
        seed_live_version_one(&fixture);
        let codes = &fixture.codes;
        // S5 of generation two measures recall 0.5 over the probe set; the
        // baseline is generation one's S5 receipt.
        let receipt = codes
            .record_probe_measurement(
                PromotionStage::AnnIndex,
                2,
                PromotionModality::Text,
                &[vec![1, 7]],
                12,
            )
            .unwrap();
        assert_eq!(receipt.recall, recall_at_k(&[1, 7], &[1, 2], 2));
        let read = codes.door.serving_read().unwrap();
        let check = codes
            .read_check_in(
                &read,
                2,
                &fixture.generation_two.model_id,
                PromotionModality::Text,
            )
            .unwrap()
            .unwrap();
        drop(read);
        assert_eq!(
            check.check.baseline, 1.0,
            "baseline is the prior Live receipt"
        );
        assert_eq!(check.model_version, 2);

        // Recall 0.5 against baseline 1.0 is a regression: the gate blocks.
        let lease = claim(&fixture);
        let refused = finalize(&fixture, &lease).unwrap_err().to_string();
        assert!(refused.contains(PROMOTION_BLOCKED), "{refused}");
        assert!(
            refused.contains("text: baseline 1 measured 0.5"),
            "{refused}"
        );
        assert_eq!(codes.live_generation().unwrap(), Some(1));
        let stored = codes
            .promotion_verdict(2)
            .unwrap()
            .expect("blocked verdict stored");
        assert!(!stored.promoted);
        assert_eq!(codes.embedding_version_of(0).unwrap(), None);

        // A re-measured S5 at full recall passes: generation two goes Live in
        // the commit that stores its verdict and tags its vectors.
        codes
            .record_probe_measurement(
                PromotionStage::AnnIndex,
                2,
                PromotionModality::Text,
                &[vec![1, 2]],
                12,
            )
            .unwrap();
        finalize(&fixture, &lease).unwrap();
        assert_eq!(codes.live_generation().unwrap(), Some(2));
        let verdict = codes.promotion_verdict(2).unwrap().unwrap();
        assert!(verdict.promoted);
        assert_eq!((verdict.model_version, verdict.entries.len()), (2, 1));
        assert_eq!(
            codes.embedding_version_of(0).unwrap(),
            Some(EmbeddingVersion::new(
                fixture.generation_two.model_id.clone(),
                2
            ))
        );
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.4
    #[test]
    fn missing_check_blocks_and_keeps_prior_live() {
        let fixture = seeded_s6_publication("promotion-gate-missing");
        seed_live_version_one(&fixture);
        let lease = claim(&fixture);
        let refused = finalize(&fixture, &lease).unwrap_err().to_string();
        // Version 2 has no stored check: the probe-set side is what is missing.
        assert!(refused.contains("text: probe set missing"), "{refused}");
        assert_eq!(fixture.codes.live_generation().unwrap(), Some(1));
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }
}
