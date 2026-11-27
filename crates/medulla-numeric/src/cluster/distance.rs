//! Distance helpers shared by every caller (MEDULLA-CLUSTER-R008.2).
//!
//! The 8-lane pairwise order is the contract: a sum has the same bits on every
//! path and thread count. The code uses no fused multiply-add and no compile-time
//! CPU flag (R007). On x86-64 the nearest-centroid scan also runs as one `avx2` function chosen at
//! run time with `is_x86_feature_detected!` (MEDULLA-CLUSTER-R007.2): it calls the same
//! `lane_reduce` body, so a host without AVX2 and a host with it produce the same bits.

/// Lane-chunked pairwise reduction `sum(term(a[i], b[i]))` over two equal-length
/// slices: eight independent accumulators, then a fixed tree, then the scalar tail.
#[inline(always)]
pub fn lane_reduce(a: &[f32], b: &[f32], term: impl Fn(f32, f32) -> f32) -> f32 {
    let mut acc = [0.0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..8 {
            acc[l] += term(x[l], y[l]);
        }
    }
    let mut s = ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]));
    for (&x, &y) in ra.iter().zip(rb) {
        s += term(x, y);
    }
    s
}

/// The squared-difference term of `sq_dist`.
#[inline(always)]
fn sq_term(x: f32, y: f32) -> f32 {
    let diff = x - y;
    diff * diff
}

/// Squared Euclidean distance between two equal-length slices.
#[inline]
pub fn sq_dist(a: &[f32], b: &[f32]) -> f32 {
    lane_reduce(a, b, sq_term)
}

/// The scan body both paths run: the one definition of the order in which candidates are
/// compared, so the baseline and the wide path cannot differ.
#[inline(always)]
fn nearest_scan(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> (usize, f32) {
    let mut best = 0usize;
    let mut bestd = f32::INFINITY;
    for c in 0..k {
        let d = lane_reduce(v, &centroids[c * dim..(c + 1) * dim], sq_term);
        if d < bestd {
            bestd = d;
            best = c;
        }
    }
    (best, bestd)
}

/// The baseline path: no CPU feature beyond the target's own baseline.
pub fn nearest_baseline(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> (usize, f32) {
    nearest_scan(v, centroids, dim, k)
}

/// The same scan compiled for AVX2. It uses no fused multiply-add: `avx2` does not enable it
/// and the compiler never contracts a float expression.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn nearest_avx2(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> (usize, f32) {
    nearest_scan(v, centroids, dim, k)
}

/// The wide path, or `None` on a host without AVX2 (or off x86-64). A test forces it here and
/// compares it with [`nearest_baseline`] (MEDULLA-CLUSTER-R007.2).
pub fn nearest_wide(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> Option<(usize, f32)> {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        // SAFETY: the running CPU reports AVX2, the one feature `nearest_avx2` requires.
        return Some(unsafe { nearest_avx2(v, centroids, dim, k) });
    }
    let _ = (v, centroids, dim, k);
    None
}

/// Index and squared distance of the nearest of `k` flattened `dim`-vectors.
/// The lowest index wins a tie. Runs the wide path when the host has AVX2.
#[inline]
pub fn nearest_with_dist(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> (usize, f32) {
    nearest_wide(v, centroids, dim, k).unwrap_or_else(|| nearest_baseline(v, centroids, dim, k))
}

/// Index of the nearest of `k` flattened `dim`-vectors to `v`.
#[inline]
pub fn nearest(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> usize {
    nearest_with_dist(v, centroids, dim, k).0
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CLUSTER-R008.2

    #[test]
    fn sq_dist_matches_scalar_reference_bits() {
        // 19 values: two full lanes of eight plus a three-value tail.
        let a: Vec<f32> = (0..19).map(|i| i as f32 * 0.37 - 2.0).collect();
        let b: Vec<f32> = (0..19).map(|i| (i * i) as f32 * 0.011 + 0.5).collect();
        let mut acc = [0.0f32; 8];
        for i in 0..16 {
            let d = a[i] - b[i];
            acc[i % 8] += d * d;
        }
        let mut s =
            ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]));
        for i in 16..19 {
            let d = a[i] - b[i];
            s += d * d;
        }
        assert_eq!(sq_dist(&a, &b).to_bits(), s.to_bits());
    }

    #[test]
    fn tie_goes_to_lowest_index() {
        let cents = [1.0f32, 1.0, 1.0, 1.0];
        assert_eq!(nearest(&[1.0, 1.0], &cents, 2, 2), 0);
    }
}
