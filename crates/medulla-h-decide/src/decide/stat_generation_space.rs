//! The embedding space of a logical graph's active generation
//! (MEDULLA-DECISION-ENGINE-R172.12.6, DECISIONS DE-R172.12.6).
//!
//! A logical graph resolves to one generation graph: the target of its
//! generation pointer, or the logical graph itself while no cycle has moved the
//! pointer. That graph's declared embedding space is the space every vector
//! over the logical graph lives in. The capability feed embeds its records in
//! it and a served caller reads it through `DecisionLog.generation_space` to
//! build a query vector in the same space. There is one resolution, here.
//!
//! A generation cycle declares the space of its shadow graph when the shadow
//! graph declares none ([`model_space`]), so a served engine that never had a
//! space declared by hand still ends a passing cycle with an active one.

use std::sync::Arc;

use medulla_decision_types::decision::statistical::retrieval_generation::{
    ActiveSpaceRead, GenerationSpaceRead, ModelRef,
};
use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_query_types::solve_scalar::digest_text;

use super::stat_generation::generation_pointer;
use super::stat_pointer::read_pointer;
use super::stat_support::invalid_parameter as invalid;
use super::SharedState;

/// Domain of a model's pinned weights identity.
const MODEL_HASH_DOMAIN: &str = "eg/generation-model/v1";
/// Domain of the engine's node-text preprocessing identity.
const PREPROCESSING_DOMAIN: &str = "eg/generation-preprocessing/v1";
/// The one preprocessing the cycle applies: a node's text, else its name, else its id.
const NODE_TEXT_PREPROCESSING: &str = "node-text";

/// The space a cycle of `model` declares on its shadow graph.
pub(super) fn model_space(model: &ModelRef) -> Result<EmbeddingSpaceRef, String> {
    EmbeddingSpaceRef::pinned(
        &model.id,
        &model.revision,
        digest_text(MODEL_HASH_DOMAIN, &(&model.id, &model.revision)),
        digest_text(PREPROCESSING_DOMAIN, &NODE_TEXT_PREPROCESSING),
        model.dimension as usize,
        false,
    )
    .map_err(invalid)
}

/// The generation graph `logical` resolves to under `tenant` (a carrier tenant scope).
pub(super) fn generation_graph(
    store: &AgentLibraryStore,
    tenant: &str,
    logical: &str,
) -> Result<String, String> {
    let (_, pointer) = read_pointer(store, tenant, &generation_pointer(logical))?;
    Ok(pointer
        .target()
        .map_or_else(|| logical.to_string(), str::to_string))
}

/// The declared space of the generation `logical` resolves to; `None` while
/// that graph declares none. An unknown graph is an error.
pub(crate) async fn active_space(
    state: &SharedState,
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    logical: &str,
) -> Result<Option<ActiveSpaceRead>, String> {
    let (blocking_store, owned_tenant, owned_logical) =
        (Arc::clone(store), tenant.to_string(), logical.to_string());
    let graph = tokio::task::spawn_blocking(move || {
        generation_graph(&blocking_store, &owned_tenant, &owned_logical)
    })
    .await
    .map_err(|error| format!("generation pointer read failed: {error}"))??;
    let guard = state.read().await;
    let entry = guard
        .registry
        .get(&graph)
        .ok_or_else(|| invalid(format!("the generation graph `{graph}` does not exist")))?;
    let store = entry.core.semantic_store.read();
    store
        .space()
        .map(|space| {
            let dimensions = u32::try_from(space.dimensions)
                .map_err(|_| invalid("the declared dimension does not fit a read"))?;
            Ok(ActiveSpaceRead {
                graph: graph.clone(),
                space_digest: space.digest.clone(),
                dimensions,
            })
        })
        .transpose()
}

/// Serve `DecisionLog.generation_space`.
pub(super) async fn serve(
    state: &SharedState,
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    logical: &str,
    model: Option<&ModelRef>,
) -> Result<GenerationSpaceRead, String> {
    if logical.is_empty() {
        return Err(invalid("a generation space read names a logical graph"));
    }
    let model_space_digest = model
        .map(model_space)
        .transpose()?
        .map(|space| space.digest);
    Ok(GenerationSpaceRead {
        logical: logical.to_string(),
        active: active_space(state, store, tenant, logical).await?,
        model_space_digest,
    })
}
