//! The durable inputs of one S6 activation (MEDULLA-UNIFIED-DATA-PLANE-R040.3).
//!
//! The engine's S6 stage worker builds the generation image and the activation artifact from
//! what S2 to S5 durably wrote: the current lexical and ANN checkpoint heads, the authoritative
//! S6 aggregate over the generation's source progress, and the vector rows of the current source
//! revision. This read proves nothing and writes nothing; `finalize_generation` re-proves every
//! predecessor inside the admitted write.

use super::checkpoint::{
    authoritative_generation_state, current_checkpoint_from_tables, GenerationCoordinates,
};
use super::record::decode;
use super::{ensure, kernel_error, refused, SemanticCodeError, SemanticCodeStore};
use medulla_index_types::semantic_index::{
    SemanticBinding, SemanticGenerationAggregate, SemanticGenerationCheckpoint, SemanticStage,
    SemanticStageIntent, SemanticVector,
};
use medulla_storage::{
    SEMANTIC_CHECKPOINTS, SEMANTIC_CHECKPOINT_HEADS, SEMANTIC_SOURCE_PROGRESS, SEMANTIC_STAGES,
    SEMANTIC_VECTORS,
};

/// What one S6 activation reads before it builds the image and the activation artifact.
pub struct S6Inputs {
    pub binding: SemanticBinding,
    pub lexical_checkpoint: SemanticGenerationCheckpoint,
    pub ann_checkpoint: SemanticGenerationCheckpoint,
    /// The S6 aggregate over the generation's current source progress.
    pub aggregate: SemanticGenerationAggregate,
    /// The vector rows of the intent's source revision, ordered by source entity id.
    pub vectors: Vec<SemanticVector>,
}

impl SemanticCodeStore {
    /// Read the S6 inputs of the generation `intent` activates.
    pub fn read_s6_inputs(&self, intent: &SemanticStageIntent) -> Result<S6Inputs, SemanticCodeError> {
        let (tenant, binding_id) = (self.tenant.as_str(), self.binding.as_str());
        let read = self.door.serving_read()?;
        let binding = self
            .read_binding_in(&read)?
            .ok_or_else(|| refused("S6 requires a durable semantic binding"))?;
        let checkpoints = read.open_owner_table(SEMANTIC_CHECKPOINTS).map_err(kernel_error)?;
        let heads = read
            .open_owner_table(SEMANTIC_CHECKPOINT_HEADS)
            .map_err(kernel_error)?;
        let at = GenerationCoordinates::of_intent(tenant, binding_id, intent);
        let head = |stage: SemanticStage, missing: &str| {
            current_checkpoint_from_tables(&checkpoints, &heads, at, stage)?
                .ok_or_else(|| refused(missing))
        };
        let lexical_checkpoint = head(
            SemanticStage::LexicalIndex,
            "S6 has no current lexical generation checkpoint",
        )?;
        let ann_checkpoint = head(
            SemanticStage::AnnIndex,
            "S6 has no current ANN generation checkpoint",
        )?;
        let (aggregate, _) = authoritative_generation_state(
            &read
                .open_owner_table(SEMANTIC_SOURCE_PROGRESS)
                .map_err(kernel_error)?,
            &read.open_owner_table(SEMANTIC_STAGES).map_err(kernel_error)?,
            at,
            SemanticStage::AnnIndex,
            SemanticStage::ReconcileAndActivate,
            None,
        )?;
        let table = read.open_owner_table(SEMANTIC_VECTORS).map_err(kernel_error)?;
        let mut vectors = Vec::new();
        for row in table
            .range((tenant, binding_id, intent.generation, "")..)
            .map_err(kernel_error)?
        {
            let (key, value) = row.map_err(kernel_error)?;
            let key = key.value();
            if (key.0, key.1, key.2) != (tenant, binding_id, intent.generation) {
                break;
            }
            let vector: SemanticVector = decode(value.value())?;
            if vector.source_revision == intent.source_revision {
                vectors.push(vector);
            }
        }
        ensure(
            vectors.len() as u64 == aggregate.completed_entity_count,
            "S6 vector rows do not cover the generation's completed entities",
        )?;
        Ok(S6Inputs {
            binding,
            lexical_checkpoint,
            ann_checkpoint,
            aggregate,
            vectors,
        })
    }
}
