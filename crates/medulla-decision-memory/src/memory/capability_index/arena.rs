//! Vector arena and re-score tier (MEDULLA-DECISION-ENGINE-R172.2.2).
//!
//! Vectors are stored L2-normalized as columnar SQ8 codes (one byte per
//! dimension, a scale and an offset per vector) beside the f32 vectors. A
//! filtered pool at or below the flat threshold gets an exact SQ8 scan; the
//! best `shortlist` entries are re-scored in f32. A larger pool goes through
//! `HnswIndex::search_filtered` with the pool as the admit predicate.

use medulla_ann::flat::Metric;
use medulla_ann::hnsw::HnswIndex;

/// Scale a vector to unit length; a zero vector stays zero.
pub fn normalized(values: &[f32]) -> Vec<f32> {
    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        values.iter().map(|v| v / norm).collect()
    } else {
        values.to_vec()
    }
}

/// The inner product of two equal-length vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Columnar vector store of one shard generation, indexed by slot.
#[derive(Debug, Clone)]
pub struct Arena {
    dim: usize,
    codes: Vec<u8>,
    scale: Vec<f32>,
    offset: Vec<f32>,
    vectors: Vec<f32>,
    graph: HnswIndex,
}

impl Arena {
    /// An empty arena of `dim` dimensions.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            codes: Vec::new(),
            scale: Vec::new(),
            offset: Vec::new(),
            vectors: Vec::new(),
            graph: HnswIndex::new(dim, Metric::InnerProduct, 16, 100, 0x5eed),
        }
    }

    /// How many slots were ever pushed, live or not.
    pub fn len(&self) -> usize {
        self.scale.len()
    }

    /// Whether no slot was pushed.
    pub fn is_empty(&self) -> bool {
        self.scale.is_empty()
    }

    /// Store `values` (normalized here) in the next slot and return it.
    pub fn push(&mut self, values: &[f32]) -> u32 {
        let slot = self.scale.len() as u32;
        let unit = normalized(values);
        let (low, high) = unit
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let scale = if high > low {
            (high - low) / 255.0
        } else {
            1.0
        };
        for value in &unit {
            self.codes
                .push((((value - low) / scale).round()).clamp(0.0, 255.0) as u8);
        }
        self.scale.push(scale);
        self.offset.push(low);
        self.vectors.extend_from_slice(&unit);
        self.graph.insert(u64::from(slot), unit);
        slot
    }

    /// Bytes held resident: the u8 codes, the per-slot scale and offset, the
    /// stored f32 vectors and the proximity graph (MEDULLA-DECISION-ENGINE-R172.10).
    pub fn resident_bytes(&self) -> usize {
        self.codes.capacity()
            + (self.scale.capacity() + self.offset.capacity() + self.vectors.capacity())
                * std::mem::size_of::<f32>()
            + self.graph.byte_size()
    }

    /// The dimension of the stored vectors.
    pub fn vector_dim(&self) -> usize {
        self.dim
    }

    /// The normalized f32 vector of `slot`.
    pub fn vector(&self, slot: u32) -> &[f32] {
        let start = slot as usize * self.dim;
        &self.vectors[start..start + self.dim]
    }

    fn approx(&self, query: &[f32], query_sum: f32, slot: u32) -> f32 {
        let start = slot as usize * self.dim;
        let codes = &self.codes[start..start + self.dim];
        let weighted: f32 = query
            .iter()
            .zip(codes)
            .map(|(q, c)| q * f32::from(*c))
            .sum();
        self.offset[slot as usize] * query_sum + self.scale[slot as usize] * weighted
    }

    /// The best `shortlist` slots of the sorted `pool` for the unit `query`,
    /// scored exactly in f32 and ordered by cosine descending, slot ascending.
    pub fn search(
        &self,
        query: &[f32],
        pool: &[u32],
        shortlist: usize,
        flat_threshold: usize,
    ) -> Vec<(u32, f32)> {
        if shortlist == 0 || pool.is_empty() {
            return Vec::new();
        }
        let mut picked: Vec<u32> = if pool.len() <= flat_threshold {
            let query_sum: f32 = query.iter().sum();
            let mut scored: Vec<(u32, f32)> = pool
                .iter()
                .map(|slot| (*slot, self.approx(query, query_sum, *slot)))
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            scored.truncate(shortlist);
            scored.into_iter().map(|(slot, _)| slot).collect()
        } else {
            let admit = |id: u64| pool.binary_search(&(id as u32)).is_ok();
            self.graph
                .search_filtered(query, shortlist, (shortlist * 4).max(64), Some(&admit))
                .into_iter()
                .map(|hit| hit.id as u32)
                .collect()
        };
        picked.sort_unstable();
        picked.dedup();
        let mut exact: Vec<(u32, f32)> = picked
            .into_iter()
            .map(|slot| (slot, dot(query, self.vector(slot))))
            .collect();
        exact.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        exact
    }
}
