//! Distance helpers shared by every caller (MEDULLA-CLUSTER-R008.2).
//!
//! The 8-lane pairwise order is the contract: a sum has the same bits on every
//! path and thread count. The code uses no fused multiply-add and no compile-time
//! CPU flag (R007).

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

/// Squared Euclidean distance between two equal-length slices.
#[inline]
pub fn sq_dist(a: &[f32], b: &[f32]) -> f32 {
    lane_reduce(a, b, |x, y| {
        let diff = x - y;
        diff * diff
    })
}

/// Index and squared distance of the nearest of `k` flattened `dim`-vectors.
/// The lowest index wins a tie.
#[inline]
pub fn nearest_with_dist(v: &[f32], centroids: &[f32], dim: usize, k: usize) -> (usize, f32) {
    let mut best = 0usize;
    let mut bestd = f32::INFINITY;
    for c in 0..k {
        let d = sq_dist(v, &centroids[c * dim..(c + 1) * dim]);
        if d < bestd {
            bestd = d;
            best = c;
        }
    }
    (best, bestd)
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
