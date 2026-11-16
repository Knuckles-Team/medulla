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
    // After a restart no graph is resident until a read asks for it, and a
    // reopened graph carries no space declaration (it is held with the
    // resident store only): open it cold and declare the space of the cycle
    // that activated it again, so the active generation survives a restart.
    // Only a catalog-known graph that is cold is opened: the feed resolves the
    // space on every sweep, and a lazy open of a graph that does not exist (or
    // is already resident) would run the open-and-evict path each time.
    let cold = {
        let guard = state.read().await;
        guard.registry.exists(&graph) && !guard.registry.is_resident(&graph)
    };
    if cold {
        medulla_server_api::api::reentry::ensure_graph_resident(state, &graph).await;
    }
    let (blocking_store, owned_tenant, owned_logical, owned_graph) = (
        Arc::clone(store),
        tenant.to_string(),
        logical.to_string(),
        graph.clone(),
    );
    let undeclared = state
        .read()
        .await
        .registry
        .get(&graph)
        .is_some_and(|entry| entry.core.semantic_store.read().space().is_none());
    let activated = if undeclared {
        tokio::task::spawn_blocking(move || {
            activated_space(&blocking_store, &owned_tenant, &owned_logical, &owned_graph)
        })
        .await
        .map_err(|error| format!("generation cycle read failed: {error}"))??
    } else {
        None
    };
    let guard = state.read().await;
    let entry = guard
        .registry
        .get(&graph)
        .ok_or_else(|| invalid(format!("the generation graph `{graph}` does not exist")))?;
    if let Some(space) = activated {
        let mut semantic = entry.core.semantic_store.write();
        if semantic.space().is_none() {
            semantic.declare_space(space)?;
        }
    }
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

/// Most generation cycle jobs one tenant's resolution scans.
const MAX_CYCLE_JOBS: usize = 4_096;

/// The space of the activated cycle whose shadow graph is `graph`, the
/// generation `logical` resolves to; `None` when no cycle activated it (the
/// logical graph itself, or a space declared by hand).
fn activated_space(
    store: &AgentLibraryStore,
    tenant: &str,
    logical: &str,
    graph: &str,
) -> Result<Option<EmbeddingSpaceRef>, String> {
    use medulla_decision_types::decision::statistical::retrieval_generation::{
        GenerationCycleJob, GenerationCycleState,
    };
    let jobs = store.decision_artifacts_with_prefix(
        tenant,
        &super::stat_generation_cycle::job_key(""),
        MAX_CYCLE_JOBS,
    )?;
    for (_, bytes) in jobs {
        let job: GenerationCycleJob = medulla_persistence_records::decision_jobs::decode_artifact(
            &bytes,
            "generation cycle",
        )?;
        let cycle = &job.cycle;
        if cycle.state() == GenerationCycleState::Activated
            && cycle.logical_graph == logical
            && cycle.shadow_graph == graph
        {
            return model_space(&cycle.model_ref).map(Some);
        }
    }
    Ok(None)
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
