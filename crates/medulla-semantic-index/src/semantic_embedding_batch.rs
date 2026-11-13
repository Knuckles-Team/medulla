//! Bounded, content-addressed batching for selective semantic regeneration.
//!
//! The cache is process-local and disposable. Its key binds the pinned
//! embedding-space digest and exact input text; it never supplies or invents
//! provenance for vectors already resident in a [`super::SemanticStore`].

use super::EmbeddingProvider;
use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_types::contract::Digest256;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum candidate count accepted by one regeneration call. This matches the
/// existing bounded semantic source page size, keeping returned vectors bounded
/// even when the caller asks the provider wrapper to batch internally.
pub const MAX_REGENERATION_CHUNKS: usize = 256;
/// Maximum combined UTF-8 source bytes accepted by one regeneration call.
pub const MAX_REGENERATION_TEXT_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRY_OVERHEAD_BYTES: usize = 128;
const MAX_NAMESPACE_COMPONENT_BYTES: usize = 256;

/// Validated authority scope for reusable embedding outputs. A cache entry can
/// be reused only for the same tenant, corpus/source scope, and encoding-policy
/// revision, in addition to the pinned model/preprocessing space and source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheNamespace {
    tenant_id: String,
    source_scope: String,
    encoding_policy_revision: String,
}

impl CacheNamespace {
    /// Build a cache namespace from the caller's validated authority coordinates.
    pub fn new(
        tenant_id: impl Into<String>,
        source_scope: impl Into<String>,
        encoding_policy_revision: impl Into<String>,
    ) -> Result<Self, String> {
        let tenant_id = tenant_id.into();
        let source_scope = source_scope.into();
        let encoding_policy_revision = encoding_policy_revision.into();
        for (field, value) in [
            ("tenant_id", &tenant_id),
            ("source_scope", &source_scope),
            ("encoding_policy_revision", &encoding_policy_revision),
        ] {
            if value.trim().is_empty() {
                return Err(format!(
                    "embedding cache namespace {field} must not be empty"
                ));
            }
            if value.len() > MAX_NAMESPACE_COMPONENT_BYTES {
                return Err(format!(
                    "embedding cache namespace {field} exceeds {MAX_NAMESPACE_COMPONENT_BYTES} bytes"
                ));
            }
        }
        Ok(Self {
            tenant_id,
            source_scope,
            encoding_policy_revision,
        })
    }
}

/// The six embedding-provider budgets (MEDULLA-FEDERATED-QUERY-R040.5.4). The
/// caller builds it from the `embed.*` registry knobs in `knob_config.rs`
/// (`embed.batch_items`, `embed.batch_bytes`, `embed.concurrency`,
/// `embed.calls_max`, `embed.cache_entries`, `embed.cache_bytes`); this layer
/// re-checks the same inclusive bounds before any provider work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbedLimits {
    /// Largest items in one provider call, `[1, 1024]`.
    pub embed_batch_items: u64,
    /// Largest source bytes in one provider call, `[4096, 67108864]`.
    pub embed_batch_bytes: u64,
    /// Provider calls in flight at once, `[1, 64]`.
    pub embed_concurrency: u64,
    /// Provider calls one run may spend, `[1, 1000000]`.
    pub embed_calls_max: u64,
    /// Resident cache entries, `[0, 10000000]`.
    pub embed_cache_entries: u64,
    /// Resident cache bytes, `[0, 4294967296]`.
    pub embed_cache_bytes: u64,
}

impl Default for EmbedLimits {
    fn default() -> Self {
        Self {
            embed_batch_items: 64,
            embed_batch_bytes: 1_048_576,
            embed_concurrency: 4,
            embed_calls_max: 1000,
            embed_cache_entries: 10_000,
            embed_cache_bytes: 67_108_864,
        }
    }
}

impl EmbedLimits {
    /// The key, value and inclusive bounds of every limit.
    pub fn entries(&self) -> [(&'static str, u64, (u64, u64)); 6] {
        [
            ("embed.batch_items", self.embed_batch_items, (1, 1024)),
            (
                "embed.batch_bytes",
                self.embed_batch_bytes,
                (4096, 67_108_864),
            ),
            ("embed.concurrency", self.embed_concurrency, (1, 64)),
            ("embed.calls_max", self.embed_calls_max, (1, 1_000_000)),
            (
                "embed.cache_entries",
                self.embed_cache_entries,
                (0, 10_000_000),
            ),
            (
                "embed.cache_bytes",
                self.embed_cache_bytes,
                (0, 4_294_967_296),
            ),
        ]
    }

    /// Refuse the first limit outside its bounds.
    pub fn validate(&self) -> Result<(), String> {
        for (key, value, (lo, hi)) in self.entries() {
            if !(lo..=hi).contains(&value) {
                return Err(format!("{key} {value} is outside [{lo}, {hi}]"));
            }
        }
        Ok(())
    }
}

/// Closed refusal set of the bounded embedding provider
/// (MEDULLA-FEDERATED-QUERY-R040.5.4). `as_str` is the wire token; every
/// refusal string the provider returns begins with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedErrorCode {
    /// A batch, byte or call ceiling would be crossed.
    BudgetExhausted,
    /// A vector of the wrong width or with a non-finite component, or the
    /// wrong number of vectors.
    VectorInvalid,
    /// The concrete provider failed.
    ProviderFailed,
}

impl EmbedErrorCode {
    /// The wire token of this refusal.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "EMBED_BUDGET_EXHAUSTED",
            Self::VectorInvalid => "EMBED_VECTOR_INVALID",
            Self::ProviderFailed => "EMBED_PROVIDER_FAILED",
        }
    }
}

fn refuse(code: EmbedErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

#[derive(Default)]
struct Lru {
    map: HashMap<Digest256, (Vec<f32>, u64)>,
    order: BTreeMap<u64, Digest256>,
    tick: u64,
}

impl Lru {
    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.saturating_add(1);
        self.tick
    }
}

/// A provider adapter that batches calls under finite budgets and reuses
/// exact, pinned-space outputs in a bounded in-memory cache. The cache is a
/// recomputable accelerator, not a store. One adapter is one run: its call
/// counter spends `embed_calls_max`.
pub struct BoundedCachingEmbeddingProvider<'a> {
    provider: &'a dyn EmbeddingProvider,
    namespace: CacheNamespace,
    space: EmbeddingSpaceRef,
    limits: EmbedLimits,
    cache_capacity: usize,
    cache: Mutex<Lru>,
    calls_used: AtomicU64,
}

impl<'a> BoundedCachingEmbeddingProvider<'a> {
    /// Wrap `provider` with the default concurrency and call budgets and an
    /// explicit batch size and cache byte budget (both checked against the
    /// `EmbedLimits` bounds).
    pub fn new(
        provider: &'a dyn EmbeddingProvider,
        namespace: CacheNamespace,
        batch_items: usize,
        cache_bytes: usize,
    ) -> Result<Self, String> {
        let limits = EmbedLimits {
            embed_batch_items: batch_items as u64,
            embed_cache_bytes: cache_bytes as u64,
            ..EmbedLimits::default()
        };
        Self::with_limits(provider, namespace, limits)
    }

    /// Wrap `provider` under the six `embed.*` knob limits. A cache
    /// budget too small for one vector disables caching while batching stays.
    pub fn with_limits(
        provider: &'a dyn EmbeddingProvider,
        namespace: CacheNamespace,
        limits: EmbedLimits,
    ) -> Result<Self, String> {
        limits.validate()?;
        let space = provider.space().clone();
        space.validate()?;
        let entry_bytes = space
            .dimensions
            .checked_mul(std::mem::size_of::<f32>())
            .and_then(|bytes| bytes.checked_add(CACHE_ENTRY_OVERHEAD_BYTES))
            .ok_or_else(|| "embedding cache entry size overflowed".to_string())?;
        let by_bytes = (limits.embed_cache_bytes / entry_bytes as u64).min(usize::MAX as u64);
        let cache_capacity = by_bytes.min(limits.embed_cache_entries) as usize;
        Ok(Self {
            provider,
            namespace,
            space,
            limits,
            cache_capacity,
            cache: Mutex::new(Lru::default()),
            calls_used: AtomicU64::new(0),
        })
    }

    /// Provider calls spent so far by this run.
    pub fn calls_used(&self) -> u64 {
        self.calls_used.load(Ordering::SeqCst)
    }

    fn cache_key(&self, text: &str) -> Result<Digest256, String> {
        let source_digest = Digest256::sha256(text.as_bytes());
        Digest256::framed(
            b"eg/semantic-embedding-cache/v1",
            &[
                self.namespace.tenant_id.as_bytes(),
                self.namespace.source_scope.as_bytes(),
                self.namespace.encoding_policy_revision.as_bytes(),
                self.space.digest.as_bytes(),
                source_digest.as_bytes(),
            ],
        )
    }

    fn touch(&self, key: Digest256) -> Option<Vec<f32>> {
        let mut cache = self.cache.lock();
        let tick = cache.next_tick();
        let (vector, used) = cache.map.get_mut(&key)?;
        let previous = std::mem::replace(used, tick);
        let vector = vector.clone();
        cache.order.remove(&previous);
        cache.order.insert(tick, key);
        Some(vector)
    }

    fn insert(&self, key: Digest256, vector: Vec<f32>) {
        if self.cache_capacity == 0 {
            return;
        }
        let mut cache = self.cache.lock();
        let tick = cache.next_tick();
        if let Some((_, previous)) = cache.map.insert(key, (vector, tick)) {
            cache.order.remove(&previous);
        }
        cache.order.insert(tick, key);
        while cache.map.len() > self.cache_capacity {
            let Some((_, oldest)) = cache.order.pop_first() else {
                break;
            };
            cache.map.remove(&oldest);
        }
    }

    fn validate_vector(&self, text_index: usize, vector: &[f32]) -> Result<(), String> {
        if vector.len() != self.space.dimensions {
            return Err(refuse(
                EmbedErrorCode::VectorInvalid,
                format!(
                    "embedding provider returned dimension {} for input {text_index}; pinned space requires {}",
                    vector.len(),
                    self.space.dimensions
                ),
            ));
        }
        if let Some(index) = vector.iter().position(|component| !component.is_finite()) {
            return Err(refuse(
                EmbedErrorCode::VectorInvalid,
                format!(
                    "embedding provider returned a non-finite component at index {index} for input {text_index}"
                ),
            ));
        }
        Ok(())
    }

    /// Split the unresolved inputs into batches that respect both the item and
    /// the byte ceiling. A single input over the byte ceiling is refused.
    fn plan_batches<'t>(
        &self,
        pending: &'t [(Digest256, usize, &'t str)],
    ) -> Result<Vec<&'t [(Digest256, usize, &'t str)]>, String> {
        let max_items = self.limits.embed_batch_items as usize;
        let max_bytes = self.limits.embed_batch_bytes as usize;
        let mut batches = Vec::new();
        let (mut start, mut bytes) = (0usize, 0usize);
        for (index, (_, _, text)) in pending.iter().enumerate() {
            if text.len() > max_bytes {
                return Err(refuse(
                    EmbedErrorCode::BudgetExhausted,
                    format!(
                        "one source of {} bytes exceeds embed_batch_bytes {max_bytes}",
                        text.len()
                    ),
                ));
            }
            if index > start && (index - start >= max_items || bytes + text.len() > max_bytes) {
                batches.push(&pending[start..index]);
                (start, bytes) = (index, 0);
            }
            bytes += text.len();
        }
        if start < pending.len() {
            batches.push(&pending[start..]);
        }
        Ok(batches)
    }
}

impl EmbeddingProvider for BoundedCachingEmbeddingProvider<'_> {
    fn space(&self) -> &EmbeddingSpaceRef {
        &self.space
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        if crate::interrupt::requested() {
            return Err("embedding regeneration was cancelled before provider work".into());
        }
        if texts.len() > MAX_REGENERATION_CHUNKS {
            return Err(refuse(
                EmbedErrorCode::BudgetExhausted,
                format!(
                    "embedding request has {} chunks; maximum is {MAX_REGENERATION_CHUNKS}",
                    texts.len()
                ),
            ));
        }
        let total_text_bytes = texts.iter().try_fold(0usize, |total, text| {
            total
                .checked_add(text.len())
                .filter(|bytes| *bytes <= MAX_REGENERATION_TEXT_BYTES)
        });
        if total_text_bytes.is_none() {
            return Err(refuse(
                EmbedErrorCode::BudgetExhausted,
                format!(
                    "embedding source text exceeds the {MAX_REGENERATION_TEXT_BYTES}-byte call budget"
                ),
            ));
        }
        if self.provider.space() != &self.space {
            return Err("embedding provider space changed after it was pinned".into());
        }
        self.space.validate()?;
        if texts.iter().any(|text| text.is_empty()) {
            return Err("embedding source text must not be empty".into());
        }
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut unique_indices = HashMap::<Digest256, usize>::with_capacity(texts.len());
        let mut unique = Vec::<(Digest256, usize, &str)>::new();
        for (index, text) in texts.iter().copied().enumerate() {
            let key = self.cache_key(text)?;
            if unique_indices.contains_key(&key) {
                continue;
            }
            unique_indices.insert(key, unique.len());
            unique.push((key, index, text));
        }

        let mut resolved = HashMap::<Digest256, Vec<f32>>::with_capacity(unique.len());
        let mut pending = Vec::<(Digest256, usize, &str)>::new();
        for (key, index, text) in unique {
            if let Some(vector) = self.touch(key) {
                resolved.insert(key, vector);
            } else {
                pending.push((key, index, text));
            }
        }

        let batches = self.plan_batches(&pending)?;
        let concurrency = self.limits.embed_concurrency as usize;
        for wave in batches.chunks(concurrency) {
            if crate::interrupt::requested() {
                return Err("embedding regeneration was cancelled between provider batches".into());
            }
            // Reserve the whole wave before any call so a refused budget has
            // no provider effect for the wave.
            let wanted = wave.len() as u64;
            let before = self.calls_used.fetch_add(wanted, Ordering::SeqCst);
            if before.saturating_add(wanted) > self.limits.embed_calls_max {
                self.calls_used.fetch_sub(wanted, Ordering::SeqCst);
                return Err(refuse(
                    EmbedErrorCode::BudgetExhausted,
                    format!(
                        "embed_calls_max {} would be crossed",
                        self.limits.embed_calls_max
                    ),
                ));
            }
            let outputs: Vec<Result<Vec<Vec<f32>>, String>> = if wave.len() == 1 {
                let texts: Vec<&str> = wave[0].iter().map(|(_, _, text)| *text).collect();
                vec![self.provider.embed_batch(&texts)]
            } else {
                std::thread::scope(|scope| {
                    let handles: Vec<_> = wave
                        .iter()
                        .map(|chunk| {
                            let texts: Vec<&str> = chunk.iter().map(|(_, _, text)| *text).collect();
                            let provider = self.provider;
                            scope.spawn(move || provider.embed_batch(&texts))
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle
                                .join()
                                .unwrap_or_else(|_| Err("provider panicked".into()))
                        })
                        .collect()
                })
            };
            for (chunk, output) in wave.iter().zip(outputs) {
                let vectors =
                    output.map_err(|detail| refuse(EmbedErrorCode::ProviderFailed, detail))?;
                if vectors.len() != chunk.len() {
                    return Err(refuse(
                        EmbedErrorCode::VectorInvalid,
                        format!(
                            "embedding provider returned {} vectors for {} requested chunks",
                            vectors.len(),
                            chunk.len()
                        ),
                    ));
                }
                for (offset, vector) in vectors.iter().enumerate() {
                    self.validate_vector(chunk[offset].1, vector)?;
                }
                for ((key, _index, _), vector) in chunk.iter().zip(vectors) {
                    self.insert(*key, vector.clone());
                    resolved.insert(*key, vector);
                }
            }
            if crate::interrupt::requested() {
                return Err("embedding regeneration was cancelled between provider batches".into());
            }
        }

        texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let key = self.cache_key(text)?;
                resolved
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| format!("embedding output missing for input {index}"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::EmbeddingChunkRef;
    use crate::semantic::EmbeddingRegenerationCandidate;
    use crate::semantic::SemanticStore;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    fn limited(
        provider: &TestProvider,
        limits: EmbedLimits,
    ) -> BoundedCachingEmbeddingProvider<'_> {
        BoundedCachingEmbeddingProvider::with_limits(provider, namespace(), limits).unwrap()
    }

    #[derive(Clone, Copy)]
    enum ProviderOutput {
        Valid,
        Error,
        WrongCardinality,
        WrongDimension,
        NonFinite,
    }

    struct TestProvider {
        space: EmbeddingSpaceRef,
        output: ProviderOutput,
        calls: AtomicUsize,
        batches: Mutex<Vec<Vec<String>>>,
        cancel_on_call: Option<Arc<AtomicBool>>,
    }

    impl TestProvider {
        fn new(output: ProviderOutput) -> Self {
            Self {
                space: EmbeddingSpaceRef::pinned(
                    "test-model",
                    "revision-1",
                    "model-content-hash",
                    "preprocessing-hash",
                    2,
                    true,
                )
                .unwrap(),
                output,
                calls: AtomicUsize::new(0),
                batches: Mutex::new(Vec::new()),
                cancel_on_call: None,
            }
        }

        fn batches(&self) -> Vec<Vec<String>> {
            self.batches.lock().clone()
        }
    }

    impl EmbeddingProvider for TestProvider {
        fn space(&self) -> &EmbeddingSpaceRef {
            &self.space
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.batches
                .lock()
                .push(texts.iter().map(|text| (*text).to_string()).collect());
            if let Some(cancel) = &self.cancel_on_call {
                cancel.store(true, Ordering::SeqCst);
            }
            match self.output {
                ProviderOutput::Error => Err("provider unavailable".into()),
                ProviderOutput::WrongCardinality => Ok(vec![vec![1.0, 0.0]]),
                ProviderOutput::WrongDimension => Ok(texts.iter().map(|_| vec![1.0]).collect()),
                ProviderOutput::NonFinite => {
                    Ok(texts.iter().map(|_| vec![1.0, f32::NAN]).collect())
                }
                ProviderOutput::Valid => Ok(texts
                    .iter()
                    .map(|text| vec![text.len() as f32, 1.0])
                    .collect()),
            }
        }
    }

    fn namespace() -> CacheNamespace {
        CacheNamespace::new("tenant-a", "corpus-a", "encoding-v1").unwrap()
    }

    fn wrapper<'a>(
        provider: &'a TestProvider,
        batch_items: usize,
        cache_bytes: usize,
    ) -> BoundedCachingEmbeddingProvider<'a> {
        BoundedCachingEmbeddingProvider::new(provider, namespace(), batch_items, cache_bytes)
            .unwrap()
    }

    #[test]
    fn batches_within_limit_coalesces_duplicates_and_reuses_cached_vectors() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        let wrapper = wrapper(&provider, 2, 4096);

        let first = wrapper.embed_batch(&["alpha", "beta", "alpha"]).unwrap();
        assert_eq!(first[0], vec![5.0, 1.0]);
        assert_eq!(first[1], vec![4.0, 1.0]);
        assert_eq!(first[0], first[2], "duplicate text shares one embedding");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider.batches(),
            vec![vec!["alpha".to_string(), "beta".to_string()]]
        );

        let repeated = wrapper.embed_batch(&["beta", "alpha"]).unwrap();
        assert_eq!(repeated, vec![first[1].clone(), first[0].clone()]);
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            1,
            "cache hit avoids a provider call"
        );
    }

    #[test]
    fn splits_provider_work_and_rejects_oversized_or_empty_sources() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        let wrapper = wrapper(&provider, 2, 0);
        let output = wrapper.embed_batch(&["a", "b", "c", "d", "e"]).unwrap();
        assert_eq!(output.len(), 5);
        assert_eq!(
            provider.batches().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
        assert!(wrapper.embed_batch(&[]).unwrap().is_empty());
        assert!(wrapper
            .embed_batch(&[""])
            .unwrap_err()
            .contains("must not be empty"));
        assert!(wrapper
            .embed_batch(&vec!["bounded"; MAX_REGENERATION_CHUNKS + 1])
            .unwrap_err()
            .contains("maximum is"));
        let oversized_source = "x".repeat(MAX_REGENERATION_TEXT_BYTES + 1);
        assert!(wrapper
            .embed_batch(&[oversized_source.as_str()])
            .unwrap_err()
            .contains("byte call budget"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn refuses_invalid_limits_and_provider_spaces() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        assert!(BoundedCachingEmbeddingProvider::new(&provider, namespace(), 0, 0).is_err());
        assert!(BoundedCachingEmbeddingProvider::new(&provider, namespace(), 1025, 0).is_err());
        assert!(
            BoundedCachingEmbeddingProvider::new(&provider, namespace(), 1, 4_294_967_297).is_err()
        );

        let wrapper = wrapper(&provider, 1, 0);
        assert_eq!(wrapper.space(), &provider.space);
    }

    #[test]
    fn cache_eviction_keeps_the_configured_entry_budget() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        // A 2-D vector plus its fixed entry allowance is 136 logical bytes.
        let wrapper = wrapper(&provider, 1, 136);
        wrapper.embed_batch(&["first"]).unwrap();
        wrapper.embed_batch(&["second"]).unwrap();
        assert_eq!(wrapper.cache.lock().map.len(), 1);
        wrapper.embed_batch(&["first"]).unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn provider_errors_cardinality_dimension_and_nonfinite_values_fail_closed() {
        for (mode, expected) in [
            (ProviderOutput::Error, "provider unavailable"),
            (ProviderOutput::WrongCardinality, "returned 1 vectors for 2"),
            (ProviderOutput::WrongDimension, "pinned space requires 2"),
            (ProviderOutput::NonFinite, "non-finite component"),
        ] {
            let provider = TestProvider::new(mode);
            let wrapper = wrapper(&provider, 2, 4096);
            let error = wrapper.embed_batch(&["one", "two"]).unwrap_err();
            assert!(error.contains(expected), "unexpected error: {error}");
        }
    }

    #[test]
    fn cancellation_between_batches_stops_work_without_returning_partial_vectors() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut provider = TestProvider::new(ProviderOutput::Valid);
        provider.cancel_on_call = Some(Arc::clone(&cancel));
        let wrapper = BoundedCachingEmbeddingProvider::with_limits(
            &provider,
            namespace(),
            EmbedLimits {
                embed_batch_items: 1,
                embed_concurrency: 1,
                ..EmbedLimits::default()
            },
        )
        .unwrap();
        let probe = Arc::clone(&cancel);
        let result = crate::interrupt::scoped(
            move || probe.load(Ordering::SeqCst),
            || wrapper.embed_batch(&["first", "second"]),
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5
    #[test]
    fn semantic_store_regeneration_uses_batched_provider_and_reuses_exact_binding() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        let wrapper = wrapper(&provider, 2, 4096);
        let space = provider.space.clone();
        let mut store = SemanticStore::new_in_space(space.clone()).unwrap();
        store
            .add_embedding("present".into(), vec![6.0, 1.0])
            .unwrap();
        let present_text = "already present";
        let missing_text = "new chunk";
        let present_identity = EmbeddingChunkRef::for_text(space.clone(), present_text);
        let candidates = [
            EmbeddingRegenerationCandidate {
                node_id: "present",
                text: present_text,
                identity: present_identity.clone(),
                existing: Some(present_identity),
            },
            EmbeddingRegenerationCandidate {
                node_id: "missing",
                text: missing_text,
                identity: EmbeddingChunkRef::for_text(space, missing_text),
                existing: None,
            },
        ];

        let report = store
            .regenerate_eligible(&candidates, Some(&wrapper))
            .unwrap();
        assert_eq!(report.reused, vec!["present"]);
        assert_eq!(report.regenerated, vec!["missing"]);
        assert_eq!(store.get_embedding("missing"), Some(vec![9.0, 1.0]));
        assert_eq!(provider.batches(), vec![vec![missing_text.to_string()]]);
    }

    #[test]
    fn cache_key_separates_tenant_source_scope_policy_and_embedding_space() {
        let provider = TestProvider::new(ProviderOutput::Valid);
        let base = wrapper(&provider, 2, 4096);
        let same_text = "shared source text";
        let base_key = base.cache_key(same_text).unwrap();

        for other_namespace in [
            CacheNamespace::new("tenant-b", "corpus-a", "encoding-v1").unwrap(),
            CacheNamespace::new("tenant-a", "corpus-b", "encoding-v1").unwrap(),
            CacheNamespace::new("tenant-a", "corpus-a", "encoding-v2").unwrap(),
        ] {
            let other =
                BoundedCachingEmbeddingProvider::new(&provider, other_namespace, 2, 4096).unwrap();
            assert_ne!(base_key, other.cache_key(same_text).unwrap());
        }

        let other_space_provider = TestProvider {
            space: EmbeddingSpaceRef::pinned(
                "test-model",
                "revision-2",
                "model-content-hash-2",
                "preprocessing-hash",
                2,
                true,
            )
            .unwrap(),
            ..TestProvider::new(ProviderOutput::Valid)
        };
        let other_space = wrapper(&other_space_provider, 2, 4096);
        assert_ne!(base_key, other_space.cache_key(same_text).unwrap());
    }

    #[test]
    fn cache_namespace_requires_each_authority_coordinate() {
        assert!(CacheNamespace::new("", "corpus", "policy-v1")
            .unwrap_err()
            .contains("tenant_id"));
        assert!(CacheNamespace::new("tenant", "  ", "policy-v1")
            .unwrap_err()
            .contains("source_scope"));
        assert!(CacheNamespace::new("tenant", "corpus", "")
            .unwrap_err()
            .contains("encoding_policy_revision"));
        assert!(CacheNamespace::new(
            "tenant",
            "corpus",
            "x".repeat(MAX_NAMESPACE_COMPONENT_BYTES + 1)
        )
        .is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5.4
    #[test]
    fn budgets_keys_and_duplicates() {
        // Duplicate rows count one provider effect.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(&provider, EmbedLimits::default());
        let out = run.embed_batch(&["a", "a", "bb", "a"]).unwrap();
        assert_eq!(out[0], out[1]);
        assert_eq!(out[0], out[3]);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(run.calls_used(), 1);
        assert_eq!(provider.batches()[0].len(), 2);

        // The item ceiling and the byte ceiling both split batches.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(
            &provider,
            EmbedLimits {
                embed_batch_items: 2,
                embed_concurrency: 1,
                ..EmbedLimits::default()
            },
        );
        run.embed_batch(&["a", "b", "c", "d", "e"]).unwrap();
        assert_eq!(
            provider.batches().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(
            &provider,
            EmbedLimits {
                embed_batch_bytes: 4096,
                embed_concurrency: 1,
                ..EmbedLimits::default()
            },
        );
        let (x, y, z) = ("x".repeat(3000), "y".repeat(3000), "z".repeat(1000));
        run.embed_batch(&[x.as_str(), y.as_str(), z.as_str()])
            .unwrap();
        assert_eq!(
            provider.batches().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 2]
        );
        let big = "w".repeat(4097);
        let refused = run.embed_batch(&[big.as_str()]).unwrap_err();
        assert!(refused.starts_with("EMBED_BUDGET_EXHAUSTED"), "{refused}");

        // Concurrency runs a wave of batches together; all calls are counted.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(
            &provider,
            EmbedLimits {
                embed_batch_items: 1,
                embed_concurrency: 2,
                ..EmbedLimits::default()
            },
        );
        run.embed_batch(&["p", "q", "r"]).unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
        assert_eq!(run.calls_used(), 3);

        // A call budget that the next wave would cross refuses with no effect.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(
            &provider,
            EmbedLimits {
                embed_batch_items: 1,
                embed_concurrency: 4,
                embed_calls_max: 1,
                ..EmbedLimits::default()
            },
        );
        let refused = run.embed_batch(&["p", "q"]).unwrap_err();
        assert!(refused.starts_with("EMBED_BUDGET_EXHAUSTED"), "{refused}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(run.calls_used(), 0);
        run.embed_batch(&["p"]).unwrap();
        assert_eq!(run.calls_used(), 1);
        assert!(run
            .embed_batch(&["q"])
            .unwrap_err()
            .starts_with("EMBED_BUDGET_EXHAUSTED"));

        // Cache hits stay inside the key: a foreign tenant, source scope, policy
        // revision or space recomputes; the same key never calls twice.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let own = limited(&provider, EmbedLimits::default());
        own.embed_batch(&["shared"]).unwrap();
        own.embed_batch(&["shared"]).unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        for other in [
            CacheNamespace::new("tenant-b", "corpus-a", "encoding-v1").unwrap(),
            CacheNamespace::new("tenant-a", "corpus-b", "encoding-v1").unwrap(),
            CacheNamespace::new("tenant-a", "corpus-a", "encoding-v2").unwrap(),
        ] {
            let before = provider.calls.load(Ordering::SeqCst);
            let foreign = BoundedCachingEmbeddingProvider::with_limits(
                &provider,
                other,
                EmbedLimits::default(),
            )
            .unwrap();
            foreign.embed_batch(&["shared"]).unwrap();
            assert_eq!(provider.calls.load(Ordering::SeqCst), before + 1);
        }

        // The entry ceiling evicts the least recently used vector.
        let provider = TestProvider::new(ProviderOutput::Valid);
        let run = limited(
            &provider,
            EmbedLimits {
                embed_cache_entries: 1,
                ..EmbedLimits::default()
            },
        );
        run.embed_batch(&["one"]).unwrap();
        run.embed_batch(&["two"]).unwrap();
        assert_eq!(run.cache.lock().map.len(), 1);
        run.embed_batch(&["one"]).unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);

        // Wrong vectors and provider failures carry their typed code.
        for (mode, code) in [
            (ProviderOutput::WrongDimension, "EMBED_VECTOR_INVALID"),
            (ProviderOutput::NonFinite, "EMBED_VECTOR_INVALID"),
            (ProviderOutput::WrongCardinality, "EMBED_VECTOR_INVALID"),
            (ProviderOutput::Error, "EMBED_PROVIDER_FAILED"),
        ] {
            let provider = TestProvider::new(mode);
            let run = limited(&provider, EmbedLimits::default());
            let refused = run.embed_batch(&["one", "two"]).unwrap_err();
            assert!(refused.starts_with(code), "{refused}");
            assert_eq!(
                run.cache.lock().map.len(),
                0,
                "a refused batch caches nothing"
            );
        }
        assert_eq!(
            EmbedErrorCode::BudgetExhausted.as_str(),
            "EMBED_BUDGET_EXHAUSTED"
        );
    }
}
