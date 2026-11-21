//! The engine's S6 stage worker (MEDULLA-UNIFIED-DATA-PLANE-R040.3).
//!
//! S1 to S5 are driven by stage workers over `Method::SemanticIndex`; S6 is run by the engine
//! itself. Once S5 publishes the generation-scoped S6 intent, this worker claims its lease on
//! its own consumer, reads what S2 to S5 durably wrote ([`SemanticCodeStore::read_s6_inputs`]),
//! builds the generation image in the pinned embedding space, the S6 transition, the S6
//! checkpoint and the activation artifact, and calls [`SemanticIndexService::activate_s6`]. The
//! promotion gate runs inside that call: a blocked promotion leaves the prior generation live.

use super::*;
use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_index_types::semantic_index::{
    SemanticActivationTarget, SemanticActivePointer, SemanticGenerationCheckpointDraft,
    SemanticGenerationDependency, SemanticStageOutcome, SemanticStagePredecessor,
    SemanticStageReceipt,
};
use medulla_semantic_index::semantic::SemanticStore;

/// The consumer the engine's S6 worker claims leases as.
pub const ENGINE_S6_WORKER: &str = "engine-s6";

/// How long the worker holds an S6 lease.
const S6_LEASE_MS: u64 = 60_000;

/// The most stage leases one run inspects.
const S6_CLAIM_LIMIT: u32 = 16;

impl SemanticIndexService {
    /// Install the S6 worker's durable subscriptions. Idempotent; call it before S5 can publish
    /// the S6 intent so the worker's consumer never starts after the row it must claim.
    pub fn subscribe_s6_worker(&self) -> Result<(), SemanticCodeError> {
        self.store.subscribe_stage_consumer(ENGINE_S6_WORKER)
    }

    /// Run the S6 stage once: claim the generation-scoped S6 lease, activate its generation and
    /// return the generation that went live. `None` means no S6 intent is waiting. A lease of
    /// another stage in the same queue class is acknowledged on this worker's own consumer (the
    /// stage workers hold their own consumers) and skipped. A failed activation, a blocked
    /// promotion included, releases the lease and returns the refusal.
    pub fn run_s6_stage(
        &self,
        space: &EmbeddingSpaceRef,
        now_ms: u64,
    ) -> Result<Option<u64>, SemanticCodeError> {
        self.subscribe_s6_worker()?;
        let mut budget = OutboxClaimBudget::new(S6_CLAIM_LIMIT, S6_LEASE_MS, now_ms)
            .map_err(SemanticCodeError::Refused)?;
        let class = SemanticStage::ReconcileAndActivate.queue_class();
        let claimed = self
            .store
            .claim_stage_class(ENGINE_S6_WORKER, class, &mut budget)?;
        for lease in claimed.claims {
            let intent = self.validate_stage_lease(&lease, &lease.consumer, now_ms)?;
            if intent.stage != SemanticStage::ReconcileAndActivate
                || !matches!(intent.scope, SemanticStageScope::Generation)
            {
                self.store.ack_stage_lease(&lease, now_ms)?;
                continue;
            }
            return match self.activate_intent(&lease, &intent, space, now_ms) {
                Ok(()) => Ok(Some(intent.generation)),
                Err(error) => {
                    let _ = self.store.release_stage_lease(&lease);
                    Err(error)
                }
            };
        }
        Ok(None)
    }

    fn activate_intent(
        &self,
        lease: &MutationOutboxLease,
        intent: &SemanticStageIntent,
        space: &EmbeddingSpaceRef,
        now_ms: u64,
    ) -> Result<(), SemanticCodeError> {
        let refuse = |message: &str| SemanticCodeError::Refused(message.to_string());
        let SemanticStagePredecessor::Activation {
            lexical_checkpoint_digest,
            ann_checkpoint_digest,
        } = &intent.predecessor
        else {
            return Err(refuse("S6 intent has no lexical/ANN activation predecessor"));
        };
        let inputs = self.store.read_s6_inputs(intent)?;
        let binding = &inputs.binding;
        if binding.model_digest != space.digest || binding.dimension as usize != space.dimensions {
            return Err(refuse(
                "the pinned embedding space is not the binding's model space",
            ));
        }
        if (
            *lexical_checkpoint_digest,
            *ann_checkpoint_digest,
        ) != (
            inputs.lexical_checkpoint.checkpoint_digest,
            inputs.ann_checkpoint.checkpoint_digest,
        ) {
            return Err(refuse("S6 predecessor is not the current lexical/ANN head"));
        }
        let mut store = SemanticStore::new_in_space(space.clone()).map_err(SemanticCodeError::Refused)?;
        for vector in inputs.vectors {
            store
                .add_embedding(vector.source_entity_id, vector.values)
                .map_err(|error| SemanticCodeError::Refused(format!("{error:?}")))?;
        }
        let image = store.export_generation()?;
        let target = SemanticActivationTarget::create(binding).map_err(semantic_contract_error)?;
        let completed_at = format!("epoch-ms:{now_ms}");
        let checkpoint = SemanticGenerationCheckpoint::create(SemanticGenerationCheckpointDraft {
            binding_id: binding.binding_id.clone(),
            binding_digest: binding.binding_digest,
            generation: intent.generation,
            source_revision: intent.source_revision.clone(),
            stage: SemanticStage::ReconcileAndActivate,
            aggregate: inputs.aggregate,
            dependency: SemanticGenerationDependency::Activation {
                lexical_checkpoint_digest: inputs.lexical_checkpoint.checkpoint_digest,
                ann_checkpoint_digest: inputs.ann_checkpoint.checkpoint_digest,
            },
            artifact_digest: target.target_digest,
            completed_at: completed_at.clone(),
        })
        .map_err(semantic_contract_error)?;
        let pointer = SemanticActivePointer {
            tenant_id: binding.tenant_id.clone(),
            binding_id: binding.binding_id.clone(),
            binding_digest: binding.binding_digest,
            generation: intent.generation,
            source_revision: intent.source_revision.clone(),
            vector_target_id: binding.vector_target_id.clone(),
            lexical_index_identity: binding.lexical_index_identity.clone(),
            ann_index_identity: binding.ann_index_identity.clone(),
            composite_policy_digest: binding.policy_digest.clone(),
            activation_receipt_digest: checkpoint.checkpoint_digest,
            activated_at: completed_at.clone(),
        };
        let artifact = SemanticGenerationArtifact::Activation {
            target: Box::new(target),
            pointer: Box::new(pointer),
        };
        artifact
            .validate_against(&checkpoint)
            .map_err(semantic_contract_error)?;
        let transition = SemanticStageTransition {
            intent: intent.clone(),
            receipt: SemanticStageReceipt {
                intent_digest: intent.intent_digest,
                output_digest: checkpoint.artifact_digest,
                cursor: "cursor:s6".to_string(),
                completed_at,
                outcome: SemanticStageOutcome::Completed,
            },
            generation_checkpoint: None,
        };
        self.activate_s6(lease, &transition, &checkpoint, &artifact, &image, now_ms)
            .map(|_| ())
    }
}
