//! medulla-ann IVF-PQ index backend for `SemanticStore` (CONCEPT:MEDULLA-KG.sharding.semantic-embedding-store-backed, feature `ann`).
//!
//! Wraps `medulla_ann::IvfPq` with the bookkeeping `SemanticStore` needs:
//!   * a `String` node-id ↔ `u64` row-id map (medulla-ann is integer-keyed),
//!   * cosine-via-normalised-L2: vectors are L2-normalised before indexing, so the
//!     IVF-PQ squared-L2 distance `d` ranks identically to cosine and the cosine
//!     similarity is recovered as `1 − d/2`,
//!   * lazy build: the index trains on the resident embeddings the first time the
//!     store crosses `ANN_BUILD_THRESHOLD`, then encodes them; below that the store
//!     uses brute force (no index).
//!
//! The index is built from the in-RAM embeddings on first use, BUT once exported
//! (`to_image`) it is restored by `from_image` WITHOUT rebuilding from raw
//! vectors — the no-rebuild behavior that distinguishes it from transient
//! indexes. The image's durable leg is an admitted `Native(SemanticIndex)`
//! mutation (`compute::semantic_ann_codes`), never a directory this crate writes.

use crate::semantic::BuildHooks;
use medulla_ann::{IvfPq, IvfPqParams, SearchParams, TrainHooks};
use medulla_numeric::cluster::{CentroidTracker, ClusterRefusal};
use std::collections::HashMap;

/// Train + switch to the IVF-PQ index once the store holds at least this many
/// embeddings. Below it, brute-force cosine is both faster and exact.
pub const ANN_BUILD_THRESHOLD: usize = 4096;
const MAX_MAINTAINED_DIMENSION: usize =
    medulla_index_types::embedding::MAX_MAINTAINED_ANN_DIMENSIONS;
pub(crate) const MAX_INDEX_ROWS: usize = 5_000_000;
const ID_MAP_MAGIC: &[u8] = b"EGIDS\x01\0";
const MAX_ID_MAP_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IDS: usize = 5_000_000;
const MAX_NODE_ID_BYTES: usize = 4_096;

/// Target IVF cells ≈ √N, clamped to a sane range for small/medium stores.
pub(crate) fn nlist_for(n: usize) -> usize {
    ((n as f64).sqrt() as usize).clamp(16, 65_536)
}

/// PQ subquantizer count: largest divisor of `dim` giving `dsub` in `[4, 16]`
/// (more subquantizers ⇒ finer codes ⇒ better recall, bounded by code size).
fn m_for(dim: usize) -> usize {
    for dsub in [4usize, 6, 8, 12, 16] {
        if dim.is_multiple_of(dsub) {
            return dim / dsub;
        }
    }
    // Fallback: any divisor.
    (1..=dim)
        .rev()
        .find(|d| dim.is_multiple_of(*d))
        .unwrap_or(1)
}

/// L2-normalise (returns the input unchanged if it is all-zero).
pub(crate) fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

/// The medulla-ann index plus the id bookkeeping `SemanticStore` needs.
pub struct AnnIndex {
    index: IvfPq,
    /// row id (u64) → node id.
    row_to_id: Vec<String>,
    /// node id → its CURRENT live row id (latest insert wins).
    id_to_row: HashMap<String, u64>,
    dim: usize,
    /// Follows the codebook as rows arrive (MEDULLA-CLUSTER-R009); `None` for an
    /// index reopened from a durable image, whose training spread is unknown.
    tracker: Option<CentroidTracker>,
    /// Drift ratio at which the index counts as stale.
    drift_threshold: f64,
}

impl AnnIndex {
    /// Train the codebooks on `sample` with the kernel (cancellable, progress-reporting).
    pub(crate) fn train_codebooks(
        sample: &[Vec<f32>],
        dim: usize,
        n: usize,
        hooks: &BuildHooks,
    ) -> Result<IvfPq, ClusterRefusal> {
        let mut params = training_params(dim, n);
        params.cluster = hooks.cluster.clone();
        IvfPq::train_with(
            &params,
            sample,
            &TrainHooks {
                cancel: &hooks.cancel,
                sink: hooks.sink.as_ref(),
            },
        )
    }

    /// Encode `rows` (id, normalized vector) into a trained index. Duplicate ids keep
    /// their last vector. Returns `None` when a row is not admissible.
    pub(crate) fn from_trained(
        mut index: IvfPq,
        sample: &[Vec<f32>],
        rows: Vec<(String, Vec<f32>)>,
        dim: usize,
        hooks: &BuildHooks,
    ) -> Option<Self> {
        let mut last: HashMap<&str, usize> = HashMap::with_capacity(rows.len());
        for (i, (id, _)) in rows.iter().enumerate() {
            last.insert(id.as_str(), i);
        }
        let mut row_to_id = Vec::with_capacity(last.len());
        let mut id_to_row = HashMap::with_capacity(last.len());
        let mut items = Vec::with_capacity(last.len());
        for (i, (id, v)) in rows.iter().enumerate() {
            if last.get(id.as_str()) != Some(&i) {
                continue;
            }
            if v.len() != dim || id.len() > MAX_NODE_ID_BYTES || v.iter().any(|x| !x.is_finite()) {
                return None;
            }
            let row = row_to_id.len() as u64;
            id_to_row.insert(id.clone(), row);
            row_to_id.push(id.clone());
            items.push((row, v.clone()));
        }
        medulla_numeric::cluster::run_in_pool(hooks.cluster.resolved_threads(), || {
            index.add(&items)
        });
        let spread = mean_coarse_distance(&index, sample);
        let tracker = CentroidTracker::new(
            index.coarse_centroids.clone(),
            dim,
            spread,
            hooks.cluster.online_window,
        );
        Some(Self {
            index,
            row_to_id,
            id_to_row,
            dim,
            tracker: Some(tracker),
            drift_threshold: hooks.cluster.refresh_drift_ratio,
        })
    }

    /// Whether ingest has drifted past `cluster.refresh_drift_ratio` since the build.
    pub fn is_stale(&self) -> bool {
        self.tracker
            .as_ref()
            .is_some_and(|t| t.is_stale(self.drift_threshold))
    }

    /// Ratio of the ingest spread to the training spread, once warmed up.
    pub fn drift_ratio(&self) -> Option<f64> {
        self.tracker.as_ref().and_then(CentroidTracker::drift_ratio)
    }

    /// Train on a sample of the embeddings and encode them all. CONCEPT:MEDULLA-KG.storage.arena-row-append — the
    /// store now holds embeddings in ONE contiguous row-major `data` buffer (`dim`
    /// floats per row) with a parallel `ids` table, so the index builds by streaming
    /// `data.chunks_exact(dim)` instead of iterating a scattered `HashMap`. `ids` must
    /// be non-empty and `data.len() == ids.len() * dim` (the resident arena).
    pub fn build(ids: &[String], data: &[f32], dim: usize) -> Option<Self> {
        validate_build_inputs(ids, data, dim)?;
        let n = ids.len();
        let hooks = BuildHooks::default();
        let sample = training_sample(data, dim, n);
        let trained = Self::train_codebooks(&sample, dim, n, &hooks).ok()?;
        let rows = data
            .chunks_exact(dim)
            .zip(ids)
            .map(|(v, id)| (id.clone(), normalize(v)))
            .collect();
        Self::from_trained(trained, &sample, rows, dim, &hooks)
    }

    /// Incrementally insert/overwrite one embedding. An overwrite tombstones the
    /// node's previous row, so search always returns the latest vector. Returns
    /// `false` (no-op) if the embedding's dim doesn't match the index.
    pub fn add(&mut self, node_id: &str, embedding: &[f32]) -> bool {
        if embedding.len() != self.dim
            || self.dim > MAX_MAINTAINED_DIMENSION
            || embedding.iter().any(|value| !value.is_finite())
            || self.index.len() >= MAX_INDEX_ROWS
            || node_id.len() > MAX_NODE_ID_BYTES
        {
            return false;
        }
        if let Some(&old) = self.id_to_row.get(node_id) {
            self.index.delete(old);
        }
        let row = self.index.len() as u64;
        let v = normalize(embedding);
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.observe(&self.index.rotate_to_index_space(&v));
        }
        self.index.add(&[(row, v)]);
        self.row_to_id.push(node_id.to_string());
        self.id_to_row.insert(node_id.to_string(), row);
        true
    }

    /// Incrementally tombstone `node_id`'s live row (CONCEPT:MEDULLA-KG.storage.incremental-ann) so
    /// search no longer returns it — no rebuild. Returns `true` if a row was
    /// tombstoned, `false` if the id was not indexed. The caller compacts once the
    /// tombstone ratio crosses the threshold.
    pub fn remove(&mut self, node_id: &str) -> bool {
        match self.id_to_row.remove(node_id) {
            Some(old) => {
                self.index.delete(old);
                true
            }
            None => false,
        }
    }

    /// kNN cosine search. Returns `(node_id, cosine_similarity)` descending.
    pub fn search(&self, query: &[f32], n_results: usize) -> Vec<(String, f32)> {
        self.search_filtered(query, n_results, |_| true)
    }

    /// kNN cosine search with a node-id metadata pre-filter (CONCEPT:MEDULLA-KG.retrieval.hybrid-metadata-prefilter). `allow`
    /// is pushed INTO the medulla-ann scan (translated node-id → external row id), so the
    /// returned top-k already satisfies the predicate rather than being over-fetched
    /// and post-filtered. Returns `(node_id, cosine_similarity)` descending.
    pub fn search_filtered(
        &self,
        query: &[f32],
        n_results: usize,
        allow: impl Fn(&str) -> bool,
    ) -> Vec<(String, f32)> {
        if query.len() != self.dim
            || self.dim > MAX_MAINTAINED_DIMENSION
            || query.iter().any(|value| !value.is_finite())
        {
            return Vec::new();
        }
        let q = normalize(query);
        let sp = SearchParams {
            nprobe: 32,
            refine: true,
            refine_factor: 16,
        };
        // medulla-ann is integer-keyed; the external id equals the `row_to_id` index (ids are
        // assigned densely and re-densified on compaction), so map id → node-id → test.
        let pred = |ext_id: u64| -> bool { allows_row(&self.row_to_id, &allow, ext_id) };
        let results = self
            .index
            .search_filtered(&q, n_results, sp, Some(&pred))
            .into_iter();
        let mut hits = Vec::new();
        for result in results {
            let Some(id) = self.row_to_id.get(result.id as usize) else {
                continue;
            };
            // squared-L2 of unit vectors = 2(1 − cos) ⇒ cos = 1 − d/2.
            hits.push((id.clone(), 1.0 - result.distance / 2.0));
        }
        hits
    }

    /// Drop tombstones via the medulla-ann compaction (VACUUM) and re-derive the id map.
    pub fn compact(&mut self) {
        let compacted = medulla_ann::compact(&self.index);
        // Rebuild the id maps over the surviving rows (compaction renumbers rows).
        let mut row_to_id = Vec::with_capacity(compacted.len());
        let mut id_to_row = HashMap::with_capacity(compacted.len());
        for (new_row, &id_u64) in compacted.ids.iter().enumerate() {
            // `ids` carries the OLD row id we assigned; map it back to the node id.
            if let Some(node) = self.row_to_id.get(id_u64 as usize) {
                id_to_row.insert(node.clone(), new_row as u64);
                row_to_id.push(node.clone());
            } else {
                row_to_id.push(String::new());
            }
        }
        // Re-key the compacted index rows to dense 0..n so future ids stay unique.
        let mut renumbered = compacted;
        for (i, slot) in renumbered.ids.iter_mut().enumerate() {
            *slot = i as u64;
        }
        self.index = renumbered;
        self.row_to_id = row_to_id;
        self.id_to_row = id_to_row;
    }

    pub fn live_len(&self) -> usize {
        self.index.live_len()
    }

    /// Width of the maintained ANN artifact.  The semantic store uses this at
    /// reopen time to reject an index trained for a different coordinate space
    /// before attaching it to resident rows.
    pub(crate) fn dim(&self) -> usize {
        self.dim
    }

    /// Integer-row id map in persisted order.  The store validates this exact
    /// member image against its durable arena before accepting a reopen.
    pub(crate) fn row_ids(&self) -> &[String] {
        &self.row_to_id
    }

    /// Tombstone fraction (drives the deferred-compaction trigger).
    pub fn tombstone_ratio(&self) -> f32 {
        self.index.tombstone_ratio()
    }
}

fn validate_build_inputs(ids: &[String], data: &[f32], dim: usize) -> Option<()> {
    if dim == 0
        || dim > MAX_MAINTAINED_DIMENSION
        || ids.is_empty()
        || ids.len() > MAX_INDEX_ROWS
        || data.len() != ids.len().checked_mul(dim)?
        || data.iter().any(|value| !value.is_finite())
        || ids.iter().any(|id| id.len() > MAX_NODE_ID_BYTES)
    {
        return None;
    }
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    if ids.iter().any(|id| !seen.insert(id)) {
        return None;
    }
    Some(())
}

fn training_sample(data: &[f32], dim: usize, n: usize) -> Vec<Vec<f32>> {
    let nlist = nlist_for(n);
    let sample_target = (nlist * 40).clamp(1, n);
    let stride = (n / sample_target).max(1);
    data.chunks_exact(dim)
        .step_by(stride)
        .map(normalize)
        .collect()
}

/// Mean squared distance of `sample` to its nearest coarse centroid.
fn mean_coarse_distance(index: &IvfPq, sample: &[Vec<f32>]) -> f64 {
    if sample.is_empty() {
        return 0.0;
    }
    let k = index.coarse_centroids.len() / index.dim.max(1);
    let total: f64 = sample
        .iter()
        .map(|v| {
            let rv = index.rotate_to_index_space(v);
            let (_, d) = medulla_numeric::cluster::nearest_with_dist(
                &rv,
                &index.coarse_centroids,
                index.dim,
                k,
            );
            f64::from(d)
        })
        .sum();
    total / sample.len() as f64
}

fn training_params(dim: usize, n: usize) -> IvfPqParams {
    IvfPqParams {
        dim,
        nlist: nlist_for(n),
        m: m_for(dim),
        cluster: Default::default(),
        opq_iters: 8,
        seed: 42,
    }
}

fn allows_row(row_to_id: &[String], allow: &impl Fn(&str) -> bool, ext_id: u64) -> bool {
    row_to_id
        .get(ext_id as usize)
        .map(|id| allow(id.as_str()))
        .unwrap_or(false)
}

#[path = "semantic_ann_backend_persistence.rs"]
mod semantic_ann_backend_persistence;

pub use semantic_ann_backend_persistence::AnnIndexImage;
