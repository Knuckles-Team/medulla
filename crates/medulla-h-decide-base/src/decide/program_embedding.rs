//! Embedding of program text for the semantic program store
//! (MEDULLA-DECISION-ENGINE-R135.16.3).
//!
//! The configured embedder is pinned as one embedding space so a graph that holds vectors of
//! another space is never mixed with these. Texts go through the bounded caching provider under
//! the six `embed.*` knobs, so one commit or lookup spends one finite call budget.

use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_query_types::solve_scalar::digest_text;
use medulla_semantic_index::semantic::{
    BoundedCachingEmbeddingProvider, CacheNamespace, EmbedLimits, EmbeddingProvider,
};
use medulla_server_ops::api::ingest::embedder::EmbedderConfig;

/// The cache scope of program text vectors.
const CACHE_SCOPE: &str = "program_lookup";

/// The space the configured embedder writes: its id is the configured model, else the embedder
/// name; its width is the configured width and its vectors are normalized.
pub(super) fn configured_space(config: &EmbedderConfig) -> Result<EmbeddingSpaceRef, String> {
    let id = config
        .model
        .clone()
        .unwrap_or_else(|| config.name().to_string());
    let model_hash = digest_text("eg/program-embedding-model/v1", &(&id, config.dims));
    let preprocessing_hash = digest_text("eg/program-embedding-preprocessing/v1", &"program-text");
    EmbeddingSpaceRef::pinned(
        id,
        "configured",
        model_hash,
        preprocessing_hash,
        config.dims,
        true,
    )
}

/// The configured embedder, bound to its pinned space.
pub struct SpacedEmbedder {
    pub config: EmbedderConfig,
    pub space: EmbeddingSpaceRef,
}

impl SpacedEmbedder {
    /// The embedder the process is configured with, and its space.
    pub fn configured() -> Result<Self, String> {
        let config = EmbedderConfig::from_env();
        let space = configured_space(&config)?;
        Ok(Self { config, space })
    }
}

impl EmbeddingProvider for SpacedEmbedder {
    fn space(&self) -> &EmbeddingSpaceRef {
        &self.space
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        self.config.embed(texts)
    }
}

/// The six `embed.*` knob values as one run's limits.
fn limits() -> EmbedLimits {
    let knobs = medulla_server_api::knob_config::knobs();
    EmbedLimits {
        embed_batch_items: knobs.embed_batch_items,
        embed_batch_bytes: knobs.embed_batch_bytes,
        embed_concurrency: knobs.embed_concurrency,
        embed_calls_max: knobs.embed_calls_max,
        embed_cache_entries: knobs.embed_cache_entries,
        embed_cache_bytes: knobs.embed_cache_bytes,
    }
}

/// Embed `texts` in order through `provider`, bounded and cached under the tenant, the
/// `program_lookup` scope and the policy digest. Blocking: call it off the reactor. Returns the
/// vectors and the provider calls spent.
pub fn embed_texts(
    provider: &dyn EmbeddingProvider,
    tenant: &str,
    policy_digest: &str,
    texts: &[&str],
) -> Result<(Vec<Vec<f32>>, u64), String> {
    let namespace = CacheNamespace::new(tenant, CACHE_SCOPE, policy_digest)?;
    let bounded = BoundedCachingEmbeddingProvider::with_limits(provider, namespace, limits())?;
    let vectors = bounded.embed_batch(texts)?;
    Ok((vectors, bounded.calls_used()))
}
