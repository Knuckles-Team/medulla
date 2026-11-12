//! Seeding (MEDULLA-CLUSTER-R002.2).
//!
//! Up to `plusplus_max_k` cells: distance-squared sampling over a seeded strided
//! sample of at most `max(8k, 4096)` rows. Above that: `k` distinct seeded rows.

use super::backend::Rows;
use super::config::ClusterConfig;
use super::distance::sq_dist;
use super::refusal::ClusterRefusal;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

/// Parallel work below this many values per call stays on the calling thread.
const PAR_THRESHOLD: usize = 1 << 16;

/// The generator every run uses: one stream per seed.
pub(super) fn rng_for(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}

/// `count` distinct ascending row indexes, one random pick per equal stride.
pub(super) fn strided_sample(rng: &mut ChaCha8Rng, n: usize, count: usize) -> Vec<u32> {
    (0..count)
        .map(|i| {
            let lo = i * n / count;
            let hi = (i + 1) * n / count;
            (lo + rng.gen_range(0..(hi - lo).max(1))) as u32
        })
        .collect()
}

fn min_update(d2: &mut [f32], sample: &[u32], rows: Rows<'_>, centre: &[f32]) {
    let update = |(slot, &i): (&mut f32, &u32)| {
        let d = sq_dist(rows.row(i as usize), centre);
        if d < *slot {
            *slot = d;
        }
    };
    if sample.len() * rows.dim >= PAR_THRESHOLD {
        d2.par_iter_mut().zip(sample.par_iter()).for_each(update);
    } else {
        d2.iter_mut().zip(sample.iter()).for_each(update);
    }
}

/// Pick the next sample position proportionally to its distance, or uniformly when
/// every sample point already coincides with a centre.
fn pick_weighted(d2: &[f32], rng: &mut ChaCha8Rng) -> usize {
    let total: f64 = d2.iter().map(|&x| f64::from(x)).sum();
    if total <= 0.0 {
        return rng.gen_range(0..d2.len());
    }
    let mut target = rng.gen::<f64>() * total;
    let mut last_positive = 0;
    for (i, &w) in d2.iter().enumerate() {
        if w > 0.0 {
            last_positive = i;
            target -= f64::from(w);
            if target <= 0.0 {
                return i;
            }
        }
    }
    last_positive
}

/// The `k` seed rows, as row indexes. Refuses `k = 0` and `k > n`.
pub fn seed_indices(
    rows: Rows<'_>,
    k: usize,
    seed: u64,
    cfg: &ClusterConfig,
) -> Result<Vec<u32>, ClusterRefusal> {
    let n = rows.n;
    if k == 0 {
        return Err(ClusterRefusal::KInvalid);
    }
    if k > n {
        return Err(ClusterRefusal::KExceedsRows { k, n });
    }
    let mut rng = rng_for(seed);
    if k > cfg.plusplus_max_k || k == n {
        return Ok(strided_sample(&mut rng, n, k));
    }
    let s = n.min((8 * k).max(4096));
    let sample = strided_sample(&mut rng, n, s);
    let first = sample[rng.gen_range(0..s)];
    let mut chosen = vec![first];
    let mut d2 = vec![f32::INFINITY; s];
    min_update(&mut d2, &sample, rows, rows.row(first as usize));
    for _ in 1..k {
        let next = sample[pick_weighted(&d2, &mut rng)];
        chosen.push(next);
        min_update(&mut d2, &sample, rows, rows.row(next as usize));
    }
    Ok(chosen)
}

/// `k * dim` seed values for `rows`: the rows named by [`seed_indices`].
pub fn seed_centers(
    rows: Rows<'_>,
    k: usize,
    seed: u64,
    cfg: &ClusterConfig,
) -> Result<Vec<f32>, ClusterRefusal> {
    let mut out = Vec::with_capacity(k * rows.dim);
    for i in seed_indices(rows, k, seed, cfg)? {
        out.extend_from_slice(rows.row(i as usize));
    }
    Ok(out)
}
