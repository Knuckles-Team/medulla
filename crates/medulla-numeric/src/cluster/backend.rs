//! The assignment backend seam (MEDULLA-CLUSTER-R008.3).
//!
//! A backend maps rows to their nearest centroid. It must return the same label as
//! the CPU path for every row. The CPU path is always compiled in.

use super::distance::nearest_with_dist;
use rayon::prelude::*;
use std::sync::{Arc, RwLock};

/// Row-major borrowed rows.
#[derive(Debug, Clone, Copy)]
pub struct Rows<'a> {
    pub data: &'a [f32],
    pub n: usize,
    pub dim: usize,
}

impl<'a> Rows<'a> {
    /// Rows view; the caller has checked `data.len() == n * dim`.
    pub fn new(data: &'a [f32], n: usize, dim: usize) -> Self {
        Self { data, n, dim }
    }

    #[inline]
    pub fn row(&self, i: usize) -> &'a [f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }
}

/// Assigns the rows named by `idx` to centroids, writing one label and one squared
/// distance per index.
pub trait AssignBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        centroids: &[f32],
        k: usize,
        labels: &mut [u32],
        dists: &mut [f32],
    );
}

/// The always-available CPU path.
pub struct CpuBackend;

impl AssignBackend for CpuBackend {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        centroids: &[f32],
        k: usize,
        labels: &mut [u32],
        dists: &mut [f32],
    ) {
        let dim = rows.dim;
        labels
            .par_iter_mut()
            .zip(dists.par_iter_mut())
            .zip(idx.par_iter())
            .for_each(|((label, dist), &i)| {
                let (c, d) = nearest_with_dist(rows.row(i as usize), centroids, dim, k);
                *label = c as u32;
                *dist = d;
            });
    }
}

static BACKEND: RwLock<Option<Arc<dyn AssignBackend>>> = RwLock::new(None);

/// Register an accelerated backend for every later run. Replaces any earlier one.
pub fn set_assign_backend(backend: Arc<dyn AssignBackend>) {
    if let Ok(mut slot) = BACKEND.write() {
        *slot = Some(backend);
    }
}

/// Drop the registered backend; runs use the CPU path again.
pub fn clear_assign_backend() {
    if let Ok(mut slot) = BACKEND.write() {
        *slot = None;
    }
}

/// The backend a run uses now.
pub fn active_backend() -> Arc<dyn AssignBackend> {
    BACKEND
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| Arc::new(CpuBackend))
}
