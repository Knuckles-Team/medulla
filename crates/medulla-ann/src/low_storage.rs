//! Low-storage ANN: revision-bound manifest, checked byte accounting, the compact
//! serving artifact and optional hub-preserving pruning.
//!
//! The artifact references source text by digest and never copies it. It adds no
//! store and no second authority: it is a byte image the durable owner stores.
//! (MEDULLA-FEDERATED-QUERY-R040.5.1, R040.5.2, R040.5.5)

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hnsw::artifact;
use crate::hnsw::compact::{CompactHnswIndex, SearchBudget, VectorProvider};
use crate::hnsw::pruning::{PruneError, PrunePolicy};
use crate::hnsw::HnswIndex;
use crate::ivfpq::{sq8_encode, SearchResult};

/// Closed set of refusal codes of the low-storage path. The ingest worker
/// reports them; they are not served-method errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowStorageErrorCode {
    ManifestInvalid,
    BytesOverflow,
    OverCeiling,
    ArtifactInvalid,
    PruningInvalid,
    PruningDisconnected,
}

impl LowStorageErrorCode {
    pub const ALL: [Self; 6] = [
        Self::ManifestInvalid,
        Self::BytesOverflow,
        Self::OverCeiling,
        Self::ArtifactInvalid,
        Self::PruningInvalid,
        Self::PruningDisconnected,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManifestInvalid => "ANN_MANIFEST_INVALID",
            Self::BytesOverflow => "ANN_BYTES_OVERFLOW",
            Self::OverCeiling => "ANN_OVER_CEILING",
            Self::ArtifactInvalid => "ANN_ARTIFACT_INVALID",
            Self::PruningInvalid => "ANN_PRUNING_INVALID",
            Self::PruningDisconnected => "ANN_PRUNING_DISCONNECTED",
        }
    }
}

/// A typed refusal naming its code and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowStorageError {
    pub code: LowStorageErrorCode,
    pub detail: String,
}

impl LowStorageError {
    fn new(code: LowStorageErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LowStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for LowStorageError {}

type Result<T> = std::result::Result<T, LowStorageError>;

/// Bytes by component. Every sum is a checked addition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageAccounting {
    pub graph_bytes: u64,
    pub code_bytes: u64,
    pub metadata_bytes: u64,
    pub source_reference_bytes: u64,
    pub cache_bytes: u64,
    pub scratch_bytes: u64,
}

impl StorageAccounting {
    fn checked_sum(parts: &[u64]) -> Result<u64> {
        parts.iter().try_fold(0u64, |acc, part| {
            acc.checked_add(*part).ok_or_else(|| {
                LowStorageError::new(
                    LowStorageErrorCode::BytesOverflow,
                    "byte accounting overflows u64",
                )
            })
        })
    }

    /// Bytes that stay after publication: graph, code, metadata, source reference.
    pub fn persisted_bytes(&self) -> Result<u64> {
        Self::checked_sum(&[
            self.graph_bytes,
            self.code_bytes,
            self.metadata_bytes,
            self.source_reference_bytes,
        ])
    }

    /// Peak bytes: every component, including cache and scratch.
    pub fn total_bytes(&self) -> Result<u64> {
        Self::checked_sum(&[
            self.graph_bytes,
            self.code_bytes,
            self.metadata_bytes,
            self.source_reference_bytes,
            self.cache_bytes,
            self.scratch_bytes,
        ])
    }

    /// Accounting from measured counts in the order graph, code, metadata,
    /// source reference, cache, scratch. A non-finite, negative or fractional
    /// count is `ANN_MANIFEST_INVALID`; a count beyond `u64` is `ANN_BYTES_OVERFLOW`.
    pub fn from_measured(counts: [f64; 6]) -> Result<Self> {
        let mut out = [0u64; 6];
        for (slot, value) in out.iter_mut().zip(counts) {
            if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
                return Err(LowStorageError::new(
                    LowStorageErrorCode::ManifestInvalid,
                    "byte counts must be finite non-negative integers",
                ));
            }
            if value >= 18_446_744_073_709_551_616.0 {
                return Err(LowStorageError::new(
                    LowStorageErrorCode::BytesOverflow,
                    "byte count exceeds u64",
                ));
            }
            *slot = value as u64;
        }
        Ok(Self {
            graph_bytes: out[0],
            code_bytes: out[1],
            metadata_bytes: out[2],
            source_reference_bytes: out[3],
            cache_bytes: out[4],
            scratch_bytes: out[5],
        })
    }
}

/// Embedding space and encoding policy one input segment was built under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentBinding {
    pub embedding_space: String,
    pub encoding_policy: String,
}

/// How hub pruning is applied. Retention is the share of the layer-0 degree
/// kept on non-hub nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TieRule {
    NodeIdAscending,
}

/// Pinned hub-preserving pruning policy, recorded in the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HubPruningPolicy {
    pub degree_threshold: usize,
    pub retention: f64,
    pub tie_rule: TieRule,
    pub revision: u64,
    pub seed_digest: [u8; 32],
}

impl HubPruningPolicy {
    /// The fixture policy: threshold 32, retention 0.5, node id ascending.
    pub fn fixture(revision: u64, seed_digest: [u8; 32]) -> Self {
        Self {
            degree_threshold: 32,
            retention: 0.5,
            tie_rule: TieRule::NodeIdAscending,
            revision,
            seed_digest,
        }
    }
}

/// Everything a manifest binds before admission.
#[derive(Clone, Debug, PartialEq)]
pub struct ManifestRequest {
    pub tenant: String,
    pub snapshot: String,
    pub source_id: String,
    pub revision: u64,
    pub text_digest: [u8; 32],
    pub embedding_space: String,
    pub encoding_policy: String,
    pub generation: u64,
    pub build_policy: String,
    pub segments: Vec<SegmentBinding>,
    pub accounting: StorageAccounting,
    pub ceiling_bytes: u64,
    pub pruning: Option<HubPruningPolicy>,
}

/// An admitted manifest. It carries the source reference (digest), never text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LowStorageAnnManifest {
    pub tenant: String,
    pub snapshot: String,
    pub source_id: String,
    pub revision: u64,
    pub text_digest: [u8; 32],
    pub embedding_space: String,
    pub encoding_policy: String,
    pub generation: u64,
    pub build_policy: String,
    pub ceiling_bytes: u64,
    pub accounting: StorageAccounting,
    /// Recorded sum of every component, equal to the checked sum.
    pub total_bytes: u64,
    pub pruning: Option<HubPruningPolicy>,
}

fn manifest_invalid(detail: &str) -> LowStorageError {
    LowStorageError::new(LowStorageErrorCode::ManifestInvalid, detail)
}

impl LowStorageAnnManifest {
    /// Validate identity, policies and byte limits before publication.
    pub fn admit(request: ManifestRequest) -> Result<Self> {
        for (name, value) in [
            ("tenant", &request.tenant),
            ("snapshot", &request.snapshot),
            ("source_id", &request.source_id),
            ("embedding_space", &request.embedding_space),
            ("encoding_policy", &request.encoding_policy),
            ("build_policy", &request.build_policy),
        ] {
            if value.trim().is_empty() {
                return Err(manifest_invalid(&format!("missing {name}")));
            }
        }
        if request.text_digest == [0u8; 32] {
            return Err(manifest_invalid("missing source text digest"));
        }
        if request.generation == 0 {
            return Err(manifest_invalid("missing source generation"));
        }
        if request.ceiling_bytes == 0 {
            return Err(manifest_invalid("ceiling must be positive"));
        }
        if request.segments.iter().any(|segment| {
            segment.embedding_space != request.embedding_space
                || segment.encoding_policy != request.encoding_policy
        }) {
            return Err(manifest_invalid(
                "segments mix embedding spaces or encoding policies",
            ));
        }
        let persisted = request.accounting.persisted_bytes()?;
        let total = request.accounting.total_bytes()?;
        if persisted > request.ceiling_bytes || total > request.ceiling_bytes {
            return Err(LowStorageError::new(
                LowStorageErrorCode::OverCeiling,
                format!(
                    "persisted {persisted} / peak {total} bytes exceed ceiling {}",
                    request.ceiling_bytes
                ),
            ));
        }
        Ok(Self {
            tenant: request.tenant,
            snapshot: request.snapshot,
            source_id: request.source_id,
            revision: request.revision,
            text_digest: request.text_digest,
            embedding_space: request.embedding_space,
            encoding_policy: request.encoding_policy,
            generation: request.generation,
            build_policy: request.build_policy,
            ceiling_bytes: request.ceiling_bytes,
            accounting: request.accounting,
            total_bytes: total,
            pruning: request.pruning,
        })
    }

    fn as_request(&self, accounting: StorageAccounting) -> ManifestRequest {
        ManifestRequest {
            tenant: self.tenant.clone(),
            snapshot: self.snapshot.clone(),
            source_id: self.source_id.clone(),
            revision: self.revision,
            text_digest: self.text_digest,
            embedding_space: self.embedding_space.clone(),
            encoding_policy: self.encoding_policy.clone(),
            generation: self.generation,
            build_policy: self.build_policy.clone(),
            segments: Vec::new(),
            accounting,
            ceiling_bytes: self.ceiling_bytes,
            pruning: self.pruning,
        }
    }

    /// Persist the manifest with its generation.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| manifest_invalid(&e.to_string()))
    }

    /// Read a manifest back and re-admit it, so a tampered sum or limit refuses.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let read: Self =
            postcard::from_bytes(bytes).map_err(|e| manifest_invalid(&e.to_string()))?;
        let admitted = Self::admit(read.as_request(read.accounting))?;
        if admitted.total_bytes != read.total_bytes {
            return Err(manifest_invalid(
                "recorded byte sum differs from components",
            ));
        }
        Ok(admitted)
    }
}

const MAGIC: &[u8; 8] = b"EGLSANN\x02";

fn artifact_invalid(detail: &str) -> LowStorageError {
    LowStorageError::new(LowStorageErrorCode::ArtifactInvalid, detail)
}

/// The compact serving artifact: vectorless graph, SQ8 codes, tombstones, ids.
#[derive(Clone, Debug)]
pub struct LowStorageArtifact {
    pub manifest: LowStorageAnnManifest,
    pub(crate) graph: CompactHnswIndex,
    pub(crate) sq_min: Vec<f32>,
    pub(crate) sq_scale: Vec<f32>,
    pub(crate) codes: Vec<u8>,
    pub(crate) tombstones: Vec<u64>,
}

struct Sq8Provider<'a> {
    artifact: &'a LowStorageArtifact,
}

impl VectorProvider for Sq8Provider<'_> {
    fn fetch(&mut self, node: usize, _external_id: u64) -> std::io::Result<Vec<f32>> {
        let a = self.artifact;
        let dim = a.graph.dim();
        let row = a
            .codes
            .get(node * dim..(node + 1) * dim)
            .ok_or_else(|| std::io::Error::other("code row missing"))?;
        let (mn, sc) = (a.sq_min[node], a.sq_scale[node]);
        Ok(row.iter().map(|c| mn + f32::from(*c) * sc).collect())
    }
}

impl HnswIndex {
    /// Build the compact artifact. The manifest's graph and code bytes are
    /// measured from the artifact and re-admitted, so over-ceiling refuses
    /// before anything is published. `tombstones` are external ids.
    pub fn to_compact(
        &self,
        mut request: ManifestRequest,
        tombstones: &[u64],
    ) -> Result<LowStorageArtifact> {
        let full =
            CompactHnswIndex::from_hnsw(self).map_err(|e| artifact_invalid(&e.to_string()))?;
        let graph_bytes = artifact::encode_compact_without_vectors(self)
            .map_err(|e| artifact_invalid(&e.to_string()))?;
        let graph = artifact::decode_compact_without_vectors(&graph_bytes)
            .map_err(|e| artifact_invalid(&e.to_string()))?;
        let dim = graph.dim();
        let vectors = full
            .vectors
            .as_ref()
            .ok_or_else(|| artifact_invalid("source graph has no vectors"))?;
        let (mut sq_min, mut sq_scale, mut codes) = (Vec::new(), Vec::new(), Vec::new());
        for row in vectors.chunks_exact(dim) {
            let (c, mn, sc) = sq8_encode(row);
            codes.extend_from_slice(&c);
            sq_min.push(mn);
            sq_scale.push(sc);
        }
        let mut tombstones = tombstones.to_vec();
        tombstones.sort_unstable();
        request.accounting.graph_bytes = graph_bytes.len() as u64;
        request.accounting.code_bytes =
            (codes.len() + (sq_min.len() + sq_scale.len()) * 4 + tombstones.len() * 8) as u64;
        let manifest = LowStorageAnnManifest::admit(request)?;
        let artifact = LowStorageArtifact {
            manifest,
            graph,
            sq_min,
            sq_scale,
            codes,
            tombstones,
        };
        artifact.validate()?;
        Ok(artifact)
    }
}

impl LowStorageArtifact {
    /// Stable external ids in node order.
    pub fn ids(&self) -> &[u64] {
        &self.graph.ids
    }

    /// Sorted tombstoned external ids.
    pub fn tombstones(&self) -> &[u64] {
        &self.tombstones
    }

    /// Structural checks shared by build and decode.
    pub fn validate(&self) -> Result<()> {
        let g = &self.graph;
        let n = g.ids.len();
        let dim = g.dim();
        if self.manifest.generation == 0 {
            return Err(artifact_invalid("missing source generation"));
        }
        if self.codes.len() != n.saturating_mul(dim)
            || self.sq_min.len() != n
            || self.sq_scale.len() != n
            || self
                .sq_min
                .iter()
                .chain(&self.sq_scale)
                .any(|v| !v.is_finite())
        {
            return Err(artifact_invalid("code shape does not match the graph"));
        }
        let mut seen = HashSet::with_capacity(n);
        if !g.ids.iter().all(|id| seen.insert(*id)) {
            return Err(artifact_invalid("duplicate stable id"));
        }
        let mut tomb = HashSet::with_capacity(self.tombstones.len());
        if !self
            .tombstones
            .iter()
            .all(|id| tomb.insert(*id) && seen.contains(id))
        {
            return Err(artifact_invalid("tombstone is duplicated or names no row"));
        }
        if g.node_layer_offsets.len() != n + 1 {
            return Err(artifact_invalid(
                "node layer offsets do not match the graph",
            ));
        }
        for node in 0..n {
            let (lo, hi) = (
                g.node_layer_offsets[node] as usize,
                g.node_layer_offsets[node + 1] as usize,
            );
            if lo >= hi || hi >= g.adjacency_offsets.len() {
                return Err(artifact_invalid("layer offsets are out of range"));
            }
            for layer in 0..(hi - lo) {
                let (a, b) = (
                    g.adjacency_offsets[lo + layer] as usize,
                    g.adjacency_offsets[lo + layer + 1] as usize,
                );
                let list = g
                    .neighbors
                    .get(a..b)
                    .ok_or_else(|| artifact_invalid("adjacency slice is out of range"))?;
                let cap = if layer == 0 { g.m0 } else { g.m };
                if list.len() > cap {
                    return Err(artifact_invalid("adjacency exceeds the degree bound"));
                }
                let mut once = HashSet::with_capacity(list.len());
                if !list
                    .iter()
                    .all(|nb| (*nb as usize) < n && *nb as usize != node && once.insert(*nb))
                {
                    return Err(artifact_invalid("dangling, self or duplicate edge"));
                }
            }
        }
        Ok(())
    }

    /// Serialise: magic (version byte 2), manifest, vectorless graph, codes,
    /// tombstones. Every section is length-prefixed.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let graph = encode_graph(&self.graph)?;
        let manifest = self.manifest.to_bytes()?;
        let mut out = MAGIC.to_vec();
        put_section(&mut out, &manifest)?;
        put_section(&mut out, &graph)?;
        let mut codes = Vec::with_capacity(self.codes.len() + self.sq_min.len() * 8);
        for i in 0..self.sq_min.len() {
            codes.extend_from_slice(&self.sq_min[i].to_le_bytes());
            codes.extend_from_slice(&self.sq_scale[i].to_le_bytes());
        }
        codes.extend_from_slice(&self.codes);
        put_section(&mut out, &codes)?;
        let tomb: Vec<u8> = self
            .tombstones
            .iter()
            .flat_map(|t| t.to_le_bytes())
            .collect();
        put_section(&mut out, &tomb)?;
        Ok(out)
    }

    /// Read an artifact back, refusing a malformed version, truncation, a shape
    /// mismatch, or a generation other than `expected_generation`.
    pub fn decode(bytes: &[u8], expected_generation: u64) -> Result<Self> {
        let rest = bytes
            .strip_prefix(&MAGIC[..])
            .ok_or_else(|| artifact_invalid("unknown artifact version"))?;
        let (manifest, rest) = take_section(rest)?;
        let (graph, rest) = take_section(rest)?;
        let (codes, rest) = take_section(rest)?;
        let (tomb, rest) = take_section(rest)?;
        if !rest.is_empty() {
            return Err(artifact_invalid("trailing bytes"));
        }
        let manifest = LowStorageAnnManifest::from_bytes(manifest)
            .map_err(|e| artifact_invalid(&e.to_string()))?;
        if manifest.generation != expected_generation {
            return Err(artifact_invalid("source generation mismatch"));
        }
        let graph = artifact::decode_compact_without_vectors(graph)
            .map_err(|e| artifact_invalid(&e.to_string()))?;
        let (n, dim) = (graph.len(), graph.dim());
        let head = n
            .checked_mul(8)
            .ok_or_else(|| artifact_invalid("code shape overflow"))?;
        if codes.len() != head + n.checked_mul(dim).unwrap_or(usize::MAX) {
            return Err(artifact_invalid("code shape does not match the graph"));
        }
        let mut sq_min = Vec::with_capacity(n);
        let mut sq_scale = Vec::with_capacity(n);
        for pair in codes[..head].chunks_exact(8) {
            sq_min.push(f32::from_le_bytes(pair[..4].try_into().expect("4 bytes")));
            sq_scale.push(f32::from_le_bytes(pair[4..].try_into().expect("4 bytes")));
        }
        if tomb.len() % 8 != 0 {
            return Err(artifact_invalid("tombstone section is truncated"));
        }
        let tombstones = tomb
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes")))
            .collect();
        let artifact = Self {
            manifest,
            graph,
            sq_min,
            sq_scale,
            codes: codes[head..].to_vec(),
            tombstones,
        };
        artifact.validate()?;
        Ok(artifact)
    }

    /// Candidate ids nearest `query`, tombstones removed, without any source
    /// fetch. The caller rescoring exactly is a separate, budgeted operation.
    pub fn candidates(&self, query: &[f32], k: usize, ef: usize) -> Result<Vec<SearchResult>> {
        let want = k.saturating_add(self.tombstones.len());
        let budget = SearchBudget {
            max_vector_fetches: self.graph.len().saturating_mul(64).max(1024),
        };
        let mut provider = Sq8Provider { artifact: self };
        let (hits, _) = self
            .graph
            .search_with(query, want, ef.max(want), budget, &mut provider)
            .map_err(|e| artifact_invalid(&e.to_string()))?;
        Ok(hits
            .into_iter()
            .filter(|h| self.tombstones.binary_search(&h.id).is_err())
            .take(k)
            .collect())
    }
}

fn encode_graph(graph: &CompactHnswIndex) -> Result<Vec<u8>> {
    artifact::encode_compact_image_of(graph).map_err(|e| artifact_invalid(&e.to_string()))
}

fn put_section(out: &mut Vec<u8>, section: &[u8]) -> Result<()> {
    let len = u32::try_from(section.len()).map_err(|_| artifact_invalid("section too large"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(section);
    Ok(())
}

fn take_section(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let head = bytes
        .get(..4)
        .ok_or_else(|| artifact_invalid("artifact is truncated"))?;
    let len = u32::from_le_bytes(head.try_into().expect("4 bytes")) as usize;
    let body = bytes
        .get(4..)
        .filter(|b| b.len() >= len)
        .ok_or_else(|| artifact_invalid("artifact is truncated"))?;
    Ok(body.split_at(len))
}

/// Hub-preserving pruning under the policy the manifest records. `None` with a
/// manifest that records no policy is the identity: the topology is
/// byte-identical. A policy that differs from the manifest's refuses.
pub fn prune_hubs(
    index: &HnswIndex,
    manifest: &LowStorageAnnManifest,
    policy: Option<&HubPruningPolicy>,
) -> Result<HnswIndex> {
    let invalid = |d: &str| LowStorageError::new(LowStorageErrorCode::PruningInvalid, d);
    let Some(policy) = policy else {
        return match manifest.pruning {
            None => Ok(index.clone()),
            Some(_) => Err(invalid(
                "manifest records a policy the caller did not supply",
            )),
        };
    };
    if manifest.pruning.as_ref() != Some(policy) {
        return Err(invalid("policy does not match the manifest"));
    }
    if policy.revision == 0 || policy.seed_digest == [0u8; 32] {
        return Err(invalid("policy needs a nonzero revision and seed digest"));
    }
    if policy.degree_threshold == 0 || !(policy.retention > 0.0 && policy.retention <= 1.0) {
        return Err(invalid("degree threshold or retention is out of bounds"));
    }
    let ordinary = ((index.m0 as f64 * policy.retention).floor() as usize).max(1);
    let work = index.len().saturating_mul(index.m0 * 40 + 64).max(1);
    let prune = PrunePolicy::new(ordinary, index.m0, policy.degree_threshold, work)
        .with_enabled(true)
        .with_provenance(policy.revision, policy.seed_digest);
    index.pruned_with_policy(prune).map_err(map_prune_error)
}

fn map_prune_error(e: PruneError) -> LowStorageError {
    match e {
        PruneError::Unreachable { .. } => {
            LowStorageError::new(LowStorageErrorCode::PruningDisconnected, e.to_string())
        }
        other => LowStorageError::new(LowStorageErrorCode::PruningInvalid, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::Metric;

    const DIM: usize = 8;

    fn vector(i: u64) -> Vec<f32> {
        let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..DIM)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x % 2000) as f32 / 100.0 - 10.0
            })
            .collect()
    }

    fn graph(n: u64) -> HnswIndex {
        let mut index = HnswIndex::new(DIM, Metric::L2, 8, 64, 7);
        for i in 0..n {
            index.insert(1000 + i, vector(i));
        }
        index
    }

    fn request() -> ManifestRequest {
        let segment = SegmentBinding {
            embedding_space: "space-a".into(),
            encoding_policy: "sq8".into(),
        };
        ManifestRequest {
            tenant: "tenant-1".into(),
            snapshot: "snap-1".into(),
            source_id: "src-1".into(),
            revision: 3,
            text_digest: [9u8; 32],
            embedding_space: "space-a".into(),
            encoding_policy: "sq8".into(),
            generation: 5,
            build_policy: "build-1".into(),
            segments: vec![segment.clone(), segment],
            accounting: StorageAccounting {
                graph_bytes: 10,
                code_bytes: 20,
                metadata_bytes: 30,
                source_reference_bytes: 40,
                cache_bytes: 50,
                scratch_bytes: 60,
            },
            ceiling_bytes: 1 << 30,
            pruning: None,
        }
    }

    fn code_of(r: Result<impl std::fmt::Debug>) -> LowStorageErrorCode {
        r.expect_err("must refuse").code
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5.1
    #[test]
    fn manifest_round_trip_and_sum() {
        let m = LowStorageAnnManifest::admit(request()).unwrap();
        // Independent oracle for the sum.
        assert_eq!(m.total_bytes, 10 + 20 + 30 + 40 + 50 + 60);
        assert_eq!(m.accounting.persisted_bytes().unwrap(), 100);
        let back = LowStorageAnnManifest::from_bytes(&m.to_bytes().unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.tenant, "tenant-1");
        assert_eq!(back.snapshot, "snap-1");
        assert_eq!(back.source_id, "src-1");
        assert_eq!(back.revision, 3);
        assert_eq!(back.text_digest, [9u8; 32]);
        assert_eq!(back.generation, 5);
        assert_eq!(back.embedding_space, "space-a");
        assert_eq!(back.encoding_policy, "sq8");
        assert_eq!(back.build_policy, "build-1");
        assert_eq!(back.ceiling_bytes, 1 << 30);

        let mut tampered = m.clone();
        tampered.total_bytes += 1;
        assert_eq!(
            code_of(LowStorageAnnManifest::from_bytes(
                &postcard::to_allocvec(&tampered).unwrap()
            )),
            LowStorageErrorCode::ManifestInvalid
        );

        let mut r = request();
        r.text_digest = [0u8; 32];
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::ManifestInvalid
        );
        let mut r = request();
        r.source_id = " ".into();
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::ManifestInvalid
        );
        let mut r = request();
        r.segments[1].encoding_policy = "other".into();
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::ManifestInvalid
        );
        let mut r = request();
        r.segments[1].embedding_space = "space-b".into();
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::ManifestInvalid
        );
        let mut r = request();
        r.accounting.scratch_bytes = u64::MAX;
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::BytesOverflow
        );
        // Persisted over ceiling.
        let mut r = request();
        r.ceiling_bytes = 99;
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::OverCeiling
        );
        // Peak (cache + scratch) over ceiling while persisted fits.
        let mut r = request();
        r.ceiling_bytes = 200;
        assert_eq!(
            code_of(LowStorageAnnManifest::admit(r)),
            LowStorageErrorCode::OverCeiling
        );
        // Non-finite counts.
        for bad in [f64::NAN, f64::INFINITY, -1.0, 1.5] {
            assert_eq!(
                code_of(StorageAccounting::from_measured([
                    bad, 0.0, 0.0, 0.0, 0.0, 0.0
                ])),
                LowStorageErrorCode::ManifestInvalid
            );
        }
        assert_eq!(
            code_of(StorageAccounting::from_measured([
                1e30, 0.0, 0.0, 0.0, 0.0, 0.0
            ])),
            LowStorageErrorCode::BytesOverflow
        );
        assert_eq!(
            StorageAccounting::from_measured([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
                .unwrap()
                .total_bytes()
                .unwrap(),
            21
        );
    }

    fn brute(index_vectors: &[(u64, Vec<f32>)], q: &[f32], k: usize) -> Vec<u64> {
        let mut all: Vec<(f32, u64)> = index_vectors
            .iter()
            .map(|(id, v)| {
                (
                    v.iter().zip(q).map(|(a, b)| (a - b) * (a - b)).sum::<f32>(),
                    *id,
                )
            })
            .collect();
        all.sort_by(|a, b| a.partial_cmp(b).unwrap());
        all.into_iter().take(k).map(|(_, id)| id).collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5.2
    #[test]
    fn compact_decode_and_refusals() {
        let n = 200u64;
        let index = graph(n);
        let tomb = [1003u64, 1010];
        let art = index.to_compact(request(), &tomb).unwrap();
        let bytes = art.encode().unwrap();
        assert_eq!(&bytes[..8], b"EGLSANN\x02");
        let back = LowStorageArtifact::decode(&bytes, 5).unwrap();
        assert_eq!(back.ids(), art.ids());
        assert_eq!(back.tombstones(), &tomb);
        assert_eq!(back.graph.edge_count(), art.graph.edge_count());
        assert_eq!(back.codes, art.codes);
        assert_eq!(back.graph.neighbors, art.graph.neighbors);
        // Accounting is measured from the artifact itself.
        assert_eq!(back.manifest.accounting.graph_bytes > 0, true);
        assert_eq!(
            back.manifest.accounting.code_bytes,
            (n as usize * DIM + n as usize * 8 + 16) as u64
        );

        // Compatibility mode (version 1 full artifact) stays readable and agrees.
        let v1 = artifact::decode(&artifact::encode(&index).unwrap()).unwrap();
        let vectors: Vec<(u64, Vec<f32>)> = (0..n).map(|i| (1000 + i, vector(i))).collect();
        let (mut hit, mut want) = (0usize, 0usize);
        for q in 0..20u64 {
            let query = vector(10_000 + q);
            let oracle: Vec<u64> = brute(&vectors, &query, 25)
                .into_iter()
                .filter(|id| !tomb.contains(id))
                .take(10)
                .collect();
            let got: Vec<u64> = back
                .candidates(&query, 10, 100)
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect();
            assert!(got.iter().all(|id| !tomb.contains(id)));
            let v1_got: Vec<u64> = v1.search(&query, 10, 100).iter().map(|h| h.id).collect();
            assert_eq!(v1_got.len(), 10);
            want += oracle.len();
            hit += oracle.iter().filter(|id| got.contains(id)).count();
        }
        assert!(hit as f64 / want as f64 >= 0.9, "recall {hit}/{want}");

        // Refusals.
        let mut bad = bytes.clone();
        bad[7] = 9;
        assert_eq!(
            code_of(LowStorageArtifact::decode(&bad, 5)),
            LowStorageErrorCode::ArtifactInvalid
        );
        assert_eq!(
            code_of(LowStorageArtifact::decode(&bytes[..bytes.len() - 3], 5)),
            LowStorageErrorCode::ArtifactInvalid
        );
        assert_eq!(
            code_of(LowStorageArtifact::decode(&bytes, 6)),
            LowStorageErrorCode::ArtifactInvalid
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            code_of(LowStorageArtifact::decode(&trailing, 5)),
            LowStorageErrorCode::ArtifactInvalid
        );

        let mutate = |f: &dyn Fn(&mut LowStorageArtifact)| {
            let mut a = art.clone();
            f(&mut a);
            code_of(a.validate().map(|_| ()))
        };
        let refused = LowStorageErrorCode::ArtifactInvalid;
        // Degree: layer-0 list longer than M0.
        assert_eq!(
            mutate(&|a| {
                let extra = a.graph.m0 as u32 + 1;
                let first = a.graph.adjacency_offsets[1] as usize;
                let add: Vec<u32> = (1..=extra).collect();
                a.graph.neighbors.splice(first..first, add);
                for o in a.graph.adjacency_offsets.iter_mut().skip(2) {
                    *o += extra;
                }
                a.graph.adjacency_offsets[1] += extra;
                let _ = first;
            }),
            refused
        );
        // Dangling edge.
        assert_eq!(mutate(&|a| a.graph.neighbors[0] = 1_000_000), refused);
        // Duplicate id.
        assert_eq!(mutate(&|a| a.graph.ids[1] = a.graph.ids[0]), refused);
        // Code shape.
        assert_eq!(
            mutate(&|a| {
                a.codes.pop();
            }),
            refused
        );
        // Tombstone naming no row.
        assert_eq!(mutate(&|a| a.tombstones.push(42)), refused);
        // Missing source generation.
        assert_eq!(mutate(&|a| a.manifest.generation = 0), refused);
        // Over ceiling refuses before the artifact exists.
        let mut r = request();
        r.ceiling_bytes = 1024;
        assert_eq!(
            code_of(index.to_compact(r, &[])),
            LowStorageErrorCode::OverCeiling
        );
        // A tombstone that names no row refuses at build.
        assert_eq!(code_of(index.to_compact(request(), &[7])), refused);
    }

    fn edge_count(index: &HnswIndex) -> usize {
        CompactHnswIndex::from_hnsw(index).unwrap().edge_count()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5.5
    #[test]
    fn pruning_disabled_identical_enabled_connected() {
        let index = graph(300);
        let m = LowStorageAnnManifest::admit(request()).unwrap();
        // Disabled: byte-identical topology.
        let same = prune_hubs(&index, &m, None).unwrap();
        assert_eq!(
            artifact::encode(&same).unwrap(),
            artifact::encode(&index).unwrap()
        );

        let mut policy = HubPruningPolicy::fixture(1, [4u8; 32]);
        policy.degree_threshold = 12;
        let mut r = request();
        r.pruning = Some(policy);
        let pm = LowStorageAnnManifest::admit(r).unwrap();
        let pruned = prune_hubs(&index, &pm, Some(&policy)).unwrap();
        let again = prune_hubs(&index, &pm, Some(&policy)).unwrap();
        assert_eq!(
            artifact::encode(&pruned).unwrap(),
            artifact::encode(&again).unwrap()
        );
        assert!(edge_count(&pruned) < edge_count(&index));
        // Valid (no dangling edge) and connected: every row is its own nearest hit.
        artifact::encode(&pruned).unwrap();
        let vectors: Vec<(u64, Vec<f32>)> = (0..300u64).map(|i| (1000 + i, vector(i))).collect();
        let found = vectors
            .iter()
            .filter(|(id, v)| pruned.search(v, 1, 64).first().map(|h| h.id) == Some(*id))
            .count();
        let recall = found as f64 / vectors.len() as f64;
        assert!(recall >= 0.95, "self recall {recall}");
        println!(
            "pruning fixture n=300 edges {} -> {} self-recall {recall:.3}",
            edge_count(&index),
            edge_count(&pruned)
        );

        // Refusals: bad bounds, missing digest, policy mismatch, work budget.
        let invalid = LowStorageErrorCode::PruningInvalid;
        for mutate in [
            (|p: &mut HubPruningPolicy| p.retention = 0.0) as fn(&mut HubPruningPolicy),
            |p| p.retention = f64::NAN,
            |p| p.degree_threshold = 0,
            |p| p.seed_digest = [0u8; 32],
            |p| p.revision = 0,
        ] {
            let mut bad = policy;
            mutate(&mut bad);
            let mut r = request();
            r.pruning = Some(bad);
            let bm = LowStorageAnnManifest::admit(r).unwrap();
            assert_eq!(code_of(prune_hubs(&index, &bm, Some(&bad))), invalid);
        }
        let mut other = policy;
        other.revision = 2;
        assert_eq!(code_of(prune_hubs(&index, &pm, Some(&other))), invalid);
        assert_eq!(code_of(prune_hubs(&index, &pm, None)), invalid);
        assert_eq!(code_of(prune_hubs(&index, &m, Some(&policy))), invalid);
        // Threshold not above the ordinary degree is an invalid bound.
        let mut low = policy;
        low.degree_threshold = 1;
        let mut r = request();
        r.pruning = Some(low);
        let lm = LowStorageAnnManifest::admit(r).unwrap();
        assert_eq!(code_of(prune_hubs(&index, &lm, Some(&low))), invalid);
        // A disconnected output is refused with its own code.
        let disconnected = map_prune_error(PruneError::Unreachable { layer: 0, node: 3 });
        assert_eq!(disconnected.code, LowStorageErrorCode::PruningDisconnected);
        assert_eq!(disconnected.code.as_str(), "ANN_PRUNING_DISCONNECTED");
    }
}
