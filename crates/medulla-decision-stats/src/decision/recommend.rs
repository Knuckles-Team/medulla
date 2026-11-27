//! Item recommendation by residual quantization and refinement
//! (MEDULLA-DECISION-ENGINE-R175.2).
//!
//! Pure and deterministic: no clock, no store, no I/O. [`fit_codebooks`]
//! learns one codebook per level by seeded k-means on the residual left by the
//! level above; the seed is the first 8 bytes of the catalog digest.
//! [`encode_item`] turns a vector into a code path. [`refine_and_rank`] moves
//! the query toward the history mean once per pause step, pulls it toward the
//! nearest item, filters to the items sharing the refined vector's code prefix
//! and orders them by cosine similarity on the Q32 scale.

use medulla_decision_outcome_types::decision::recommend::{
    RecommendedItem, SemanticId, MAX_PAUSE_STEPS,
};
use medulla_decision_types::decision::knob_families::recommend::RecommendKnobs;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// The most levels one semantic id carries.
pub const MAX_LEVELS: usize = 4;
const Q32_ONE: f64 = 4_294_967_296.0;

/// Why a recommendation step is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendError {
    EmptyCatalog,
    DimensionMismatch,
    /// A knob is outside the range its spec admits.
    KnobOutOfRange,
}

/// One catalog item with its embedding and time bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogItem {
    pub item_id: String,
    pub time_bucket: u16,
    pub vector: Vec<f32>,
}

/// One catalog item with its semantic id.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedItem {
    pub item_id: String,
    pub semantic_id: SemanticId,
    pub vector: Vec<f32>,
}

/// The fitted residual-quantization codebooks of one catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Encoder {
    pub dimension: usize,
    /// `codebooks[level][code]` is a centroid of `dimension` values.
    pub codebooks: Vec<Vec<Vec<f64>>>,
}

fn check_knobs(knobs: &RecommendKnobs) -> Result<(), RecommendError> {
    let ok = (1..=MAX_LEVELS).contains(&knobs.levels)
        && (2..=256).contains(&knobs.codes_per_level)
        && knobs.kmeans_iters >= 1
        && (0.0..=1.0).contains(&knobs.pause_alpha)
        && (0.0..=1.0).contains(&knobs.nearest_pull);
    if ok {
        Ok(())
    } else {
        Err(RecommendError::KnobOutOfRange)
    }
}

/// The k-means seed of a catalog: the first 8 bytes of its digest, big endian.
pub fn seed_of(catalog_digest: &[u8; 32]) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&catalog_digest[..8]);
    u64::from_be_bytes(word)
}

fn squared_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// The index of the centroid nearest `point`; the lowest index wins a tie.
fn nearest(centroids: &[Vec<f64>], point: &[f64]) -> usize {
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    for (index, centroid) in centroids.iter().enumerate() {
        let distance = squared_distance(centroid, point);
        if distance < best_distance {
            best = index;
            best_distance = distance;
        }
    }
    best
}

fn widen(vector: &[f32]) -> Vec<f64> {
    vector.iter().map(|value| f64::from(*value)).collect()
}

/// `count` distinct indices below `len`, by a seeded partial shuffle.
fn seeded_picks(rng: &mut ChaCha8Rng, len: usize, count: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..len).collect();
    for position in 0..count {
        let remaining = (len - position) as u64;
        let swap = position + (rng.next_u64() % remaining) as usize;
        indices.swap(position, swap);
    }
    indices.truncate(count);
    indices
}

/// One level of seeded k-means over `residuals`.
fn kmeans(
    rng: &mut ChaCha8Rng,
    residuals: &[Vec<f64>],
    codes: usize,
    iterations: usize,
) -> Vec<Vec<f64>> {
    let count = codes.min(residuals.len()).max(1);
    let mut centroids: Vec<Vec<f64>> = seeded_picks(rng, residuals.len(), count)
        .into_iter()
        .map(|index| residuals[index].clone())
        .collect();
    let dimension = residuals[0].len();
    for _ in 0..iterations {
        let mut sums = vec![vec![0.0f64; dimension]; count];
        let mut sizes = vec![0usize; count];
        for residual in residuals {
            let code = nearest(&centroids, residual);
            sizes[code] += 1;
            for (slot, value) in sums[code].iter_mut().zip(residual) {
                *slot += value;
            }
        }
        let mut moved = false;
        for code in 0..count {
            if sizes[code] == 0 {
                continue;
            }
            let mean: Vec<f64> = sums[code]
                .iter()
                .map(|sum| sum / sizes[code] as f64)
                .collect();
            if mean != centroids[code] {
                moved = true;
                centroids[code] = mean;
            }
        }
        if !moved {
            break;
        }
    }
    centroids
}

/// Fit one codebook per level on the residuals of the level above. Every item
/// must share one dimension; the seed is the first 8 bytes of `catalog_digest`.
pub fn fit_codebooks(
    items: &[CatalogItem],
    catalog_digest: &[u8; 32],
    knobs: &RecommendKnobs,
) -> Result<Encoder, RecommendError> {
    check_knobs(knobs)?;
    let first = items.first().ok_or(RecommendError::EmptyCatalog)?;
    let dimension = first.vector.len();
    if dimension == 0 || items.iter().any(|item| item.vector.len() != dimension) {
        return Err(RecommendError::DimensionMismatch);
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed_of(catalog_digest));
    let mut residuals: Vec<Vec<f64>> = items.iter().map(|item| widen(&item.vector)).collect();
    let mut codebooks = Vec::with_capacity(knobs.levels);
    for _ in 0..knobs.levels {
        let centroids = kmeans(
            &mut rng,
            &residuals,
            knobs.codes_per_level,
            knobs.kmeans_iters,
        );
        for residual in &mut residuals {
            let code = nearest(&centroids, residual);
            for (value, centre) in residual.iter_mut().zip(&centroids[code]) {
                *value -= centre;
            }
        }
        codebooks.push(centroids);
    }
    Ok(Encoder {
        dimension,
        codebooks,
    })
}

/// The code path of `vector`: the nearest centroid of each level, subtracted
/// before the next. Levels the encoder lacks stay 0. O(L*C*d).
pub fn encode_item(encoder: &Encoder, vector: &[f32]) -> Result<[u8; MAX_LEVELS], RecommendError> {
    if vector.len() != encoder.dimension {
        return Err(RecommendError::DimensionMismatch);
    }
    let mut residual = widen(vector);
    let mut codes = [0u8; MAX_LEVELS];
    for (level, centroids) in encoder.codebooks.iter().take(MAX_LEVELS).enumerate() {
        let code = nearest(centroids, &residual);
        codes[level] = u8::try_from(code).map_err(|_| RecommendError::KnobOutOfRange)?;
        for (value, centre) in residual.iter_mut().zip(&centroids[code]) {
            *value -= centre;
        }
    }
    Ok(codes)
}

/// Encode a whole catalog with the frozen codebooks.
pub fn encode_catalog(
    encoder: &Encoder,
    items: &[CatalogItem],
) -> Result<Vec<EncodedItem>, RecommendError> {
    items
        .iter()
        .map(|item| {
            Ok(EncodedItem {
                item_id: item.item_id.clone(),
                semantic_id: SemanticId {
                    time_bucket: item.time_bucket,
                    codes: encode_item(encoder, &item.vector)?,
                },
                vector: item.vector.clone(),
            })
        })
        .collect()
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm =
        a.iter().map(|x| x * x).sum::<f64>().sqrt() * b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm == 0.0 {
        0.0
    } else {
        (dot / norm).clamp(-1.0, 1.0)
    }
}

/// Cosine similarity on the Q32 scale.
fn q32(value: f64) -> i64 {
    (value * Q32_ONE).round() as i64
}

/// Refine the query by `pause_steps`, filter to its code-prefix bucket and
/// order by cosine on the Q32 scale (ties by item id). The bucket drops its
/// last level while it holds fewer than `top_k` items.
pub fn refine_and_rank(
    encoder: &Encoder,
    catalog: &[EncodedItem],
    query: &[f32],
    history_item_ids: &[String],
    pause_steps: u8,
    top_k: u16,
    knobs: &RecommendKnobs,
) -> Result<Vec<RecommendedItem>, RecommendError> {
    check_knobs(knobs)?;
    if catalog.is_empty() {
        return Err(RecommendError::EmptyCatalog);
    }
    if query.len() != encoder.dimension
        || catalog
            .iter()
            .any(|item| item.vector.len() != encoder.dimension)
        || pause_steps > MAX_PAUSE_STEPS
    {
        return Err(RecommendError::DimensionMismatch);
    }
    let vectors: Vec<Vec<f64>> = catalog.iter().map(|item| widen(&item.vector)).collect();
    let history: Vec<&Vec<f64>> = catalog
        .iter()
        .zip(&vectors)
        .filter(|(item, _)| history_item_ids.iter().any(|id| id == &item.item_id))
        .map(|(_, vector)| vector)
        .collect();
    let mut working = widen(query);
    if !history.is_empty() {
        let mut mean = vec![0.0f64; encoder.dimension];
        for vector in &history {
            for (slot, value) in mean.iter_mut().zip(vector.iter()) {
                *slot += value;
            }
        }
        for slot in &mut mean {
            *slot /= history.len() as f64;
        }
        for _ in 0..pause_steps {
            for (value, target) in working.iter_mut().zip(&mean) {
                *value += knobs.pause_alpha * (target - *value);
            }
            let mut best = 0;
            let mut best_score = f64::NEG_INFINITY;
            for (index, vector) in vectors.iter().enumerate() {
                let score = cosine(&working, vector);
                if score > best_score {
                    best = index;
                    best_score = score;
                }
            }
            for (value, target) in working.iter_mut().zip(&vectors[best]) {
                *value += knobs.nearest_pull * (target - *value);
            }
        }
    }
    let refined: Vec<f32> = working.iter().map(|value| *value as f32).collect();
    let codes = encode_item(encoder, &refined)?;
    let wanted = usize::from(top_k);
    let depth_max = encoder.codebooks.len().min(MAX_LEVELS);
    let mut depth = depth_max;
    let bucket = |depth: usize| -> Vec<usize> {
        (0..catalog.len())
            .filter(|index| catalog[*index].semantic_id.codes[..depth] == codes[..depth])
            .collect()
    };
    let mut members = bucket(depth);
    while members.len() < wanted && depth > 0 {
        depth -= 1;
        members = bucket(depth);
    }
    let mut scored: Vec<(i64, usize)> = members
        .into_iter()
        .map(|index| (q32(cosine(&working, &vectors[index])), index))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| catalog[a.1].item_id.cmp(&catalog[b.1].item_id))
    });
    Ok(scored
        .into_iter()
        .take(wanted)
        .map(|(score_q32, index)| RecommendedItem {
            item_id: catalog[index].item_id.clone(),
            semantic_id: catalog[index].semantic_id,
            score_q32,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knobs() -> RecommendKnobs {
        RecommendKnobs {
            levels: 2,
            codes_per_level: 4,
            kmeans_iters: 10,
            pause_alpha: 0.5,
            nearest_pull: 0.25,
        }
    }

    /// Four clusters of six items on the axes of a 4-dimensional space.
    pub(crate) fn clustered_catalog() -> Vec<CatalogItem> {
        let mut items = Vec::new();
        for cluster in 0..4usize {
            for member in 0..6usize {
                let mut vector = vec![0.05f32 * member as f32; 4];
                vector[cluster] = 1.0 + 0.01 * member as f32;
                items.push(CatalogItem {
                    item_id: format!("c{cluster}-i{member}"),
                    time_bucket: 0,
                    vector,
                });
            }
        }
        items
    }

    fn digest() -> [u8; 32] {
        let mut digest = [0u8; 32];
        digest[..8].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_be_bytes());
        digest
    }

    fn rank(pause_steps: u8, top_k: u16) -> Vec<RecommendedItem> {
        let items = clustered_catalog();
        let encoder = fit_codebooks(&items, &digest(), &knobs()).expect("fit");
        let catalog = encode_catalog(&encoder, &items).expect("encode");
        refine_and_rank(
            &encoder,
            &catalog,
            &[0.9, 0.2, 0.0, 0.0],
            &["c0-i0".to_string()],
            pause_steps,
            top_k,
            &knobs(),
        )
        .expect("rank")
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.2
    #[test]
    fn seeded_fixture_ranks_identically_twice() {
        let items = clustered_catalog();
        let first = fit_codebooks(&items, &digest(), &knobs()).expect("fit");
        let second = fit_codebooks(&items, &digest(), &knobs()).expect("fit");
        assert_eq!(first, second);
        assert_eq!(rank(2, 6), rank(2, 6));
        assert!(!rank(2, 6).is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.2
    #[test]
    fn empty_bucket_widens_by_one_level() {
        let items = clustered_catalog();
        let encoder = fit_codebooks(&items, &digest(), &knobs()).expect("fit");
        let mut catalog = encode_catalog(&encoder, &items).expect("encode");
        // Move every item off the query's full code path so the two-level
        // bucket is empty and only the one-level prefix can fill it.
        let query = [0.9f32, 0.2, 0.0, 0.0];
        let path = encode_item(&encoder, &query).expect("path");
        for item in &mut catalog {
            if item.semantic_id.codes[..2] == path[..2] {
                item.semantic_id.codes[1] = item.semantic_id.codes[1].wrapping_add(1);
            }
        }
        let ranked =
            refine_and_rank(&encoder, &catalog, &query, &[], 0, 3, &knobs()).expect("rank");
        assert!(!ranked.is_empty());
        assert!(ranked
            .iter()
            .all(|item| item.semantic_id.codes[0] == path[0]));
    }
}
