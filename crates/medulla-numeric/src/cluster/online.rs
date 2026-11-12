//! Online centroid tracking and the drift ratio (MEDULLA-CLUSTER-R009.1, R009.2).

use super::distance::nearest_with_dist;

/// Follows a built codebook as new rows arrive.
#[derive(Debug, Clone)]
pub struct CentroidTracker {
    dim: usize,
    centroids: Vec<f32>,
    counts: Vec<u64>,
    window: u64,
    training_inertia: f64,
    ingest_inertia: f64,
    observed: u64,
}

impl CentroidTracker {
    /// Track `centroids` (`k * dim`) built with a mean squared distance of
    /// `training_mean_sq_dist`. `window` is the moving-average window in points.
    pub fn new(centroids: Vec<f32>, dim: usize, training_mean_sq_dist: f64, window: u64) -> Self {
        let k = centroids.len() / dim.max(1);
        Self {
            dim,
            centroids,
            counts: vec![window.max(1); k],
            window: window.max(1),
            training_inertia: training_mean_sq_dist,
            ingest_inertia: training_mean_sq_dist,
            observed: 0,
        }
    }

    /// Assign `row` to its nearest centroid, move that centroid toward it, and
    /// fold the squared distance into the ingest average. Returns the cell.
    pub fn observe(&mut self, row: &[f32]) -> usize {
        let k = self.counts.len();
        let (c, d) = nearest_with_dist(row, &self.centroids, self.dim, k);
        self.counts[c] = (self.counts[c] + 1).min(self.window);
        let eta = (1.0 / self.counts[c] as f64).max(1.0 / self.window as f64);
        for (x, &v) in self.centroids[c * self.dim..(c + 1) * self.dim]
            .iter_mut()
            .zip(row)
        {
            *x = (f64::from(*x) + eta * (f64::from(v) - f64::from(*x))) as f32;
        }
        let a = 1.0 / self.window as f64;
        self.ingest_inertia += a * (f64::from(d) - self.ingest_inertia);
        self.observed += 1;
        c
    }

    /// Ingest inertia over training inertia; `None` until `window / 4` rows arrived.
    pub fn drift_ratio(&self) -> Option<f64> {
        if self.observed < self.window / 4 || self.training_inertia <= 0.0 {
            return None;
        }
        Some(self.ingest_inertia / self.training_inertia)
    }

    /// Whether the drift ratio is at or above `threshold`.
    pub fn is_stale(&self, threshold: f64) -> bool {
        self.drift_ratio().is_some_and(|r| r >= threshold)
    }

    /// The current centroids, the warm start of a retrain.
    pub fn centroids(&self) -> &[f32] {
        &self.centroids
    }

    pub fn observed(&self) -> u64 {
        self.observed
    }
}
