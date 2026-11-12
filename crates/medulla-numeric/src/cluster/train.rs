//! The training loop: mini-batch moving-average steps and the exact mode
//! (MEDULLA-CLUSTER-R002.3, R002.4, R003.1, R005.2, R005.3).
//!
//! Assignment is a per-row map and never reduces floats. The fold of batch rows
//! into centroids is sequential, in draw order, so the bits do not depend on the
//! thread count.

use super::backend::{active_backend, AssignBackend, Rows};
use super::config::ClusterConfig;
use super::plan::{Mode, Plan};
use super::refusal::ClusterRefusal;
use super::seed::rng_for;
use super::state::{CancelToken, ClusterRun, Event};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Mixed into the seed so the batch stream differs from the seeding stream.
pub(super) const TRAIN_STREAM: u64 = 0x9E37_79B9_7F4A_7C15;

/// How a finished run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Converged,
    BudgetExhausted,
}

/// What a finished run returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterOutcome {
    /// `k * dim` centroid values.
    pub centroids: Vec<f32>,
    /// One label per row; empty when the run did not ask for labels.
    pub labels: Vec<u32>,
    pub end: End,
    pub steps: u32,
    /// Sum of squared distances to the nearest centroid.
    pub inertia: f64,
    /// `inertia / n`.
    pub mean_sq_dist: f64,
}

/// Scratch arrays of one run, allocated once (the formula of `training_bytes`).
struct Scratch {
    idx: Vec<u32>,
    labels: Vec<u32>,
    dists: Vec<f32>,
    order: Vec<u32>,
    counts: Vec<u64>,
    streak: Vec<u32>,
    m: Vec<u32>,
    offsets: Vec<u32>,
    cursor: Vec<u32>,
    acc: Vec<f64>,
}

impl Scratch {
    fn new(k: usize, dim: usize, batch: usize) -> Self {
        Self {
            idx: vec![0; batch],
            labels: vec![0; batch],
            dists: vec![0.0; batch],
            order: vec![0; batch],
            counts: vec![1; k],
            streak: vec![0; k],
            m: vec![0; k],
            offsets: vec![0; k + 1],
            cursor: vec![0; k + 1],
            acc: vec![0.0; dim],
        }
    }
}

pub(super) struct Trainer<'a> {
    pub rows: Rows<'a>,
    pub k: usize,
    pub cfg: &'a ClusterConfig,
    pub plan: Plan,
    pub cancel: &'a CancelToken,
    pub backend: std::sync::Arc<dyn AssignBackend>,
}

impl Trainer<'_> {
    /// Rows per assignment chunk so one chunk is about 2^27 multiply-adds.
    fn chunk_rows(&self) -> usize {
        let per_row = (self.k * self.rows.dim).max(1);
        ((1usize << 27) / per_row).clamp(256, 65_536)
    }

    fn check_cancel(&self) -> Result<(), ClusterRefusal> {
        if self.cancel.is_cancelled() {
            Err(ClusterRefusal::Cancelled)
        } else {
            Ok(())
        }
    }

    fn assign_chunked(
        &self,
        idx: &[u32],
        centroids: &[f32],
        labels: &mut [u32],
        dists: &mut [f32],
    ) -> Result<(), ClusterRefusal> {
        let chunk = self.chunk_rows();
        for ((i, l), d) in idx
            .chunks(chunk)
            .zip(labels.chunks_mut(chunk))
            .zip(dists.chunks_mut(chunk))
        {
            self.check_cancel()?;
            self.backend.assign(self.rows, i, centroids, self.k, l, d);
        }
        Ok(())
    }

    fn draw(&self, s: &mut Scratch, rng: &mut ChaCha8Rng) {
        match self.plan.mode {
            Mode::Exact => s
                .idx
                .iter_mut()
                .enumerate()
                .for_each(|(i, slot)| *slot = i as u32),
            Mode::EMeans => {
                let n = self.rows.n;
                s.idx
                    .iter_mut()
                    .for_each(|slot| *slot = rng.gen_range(0..n) as u32);
            }
        }
    }

    /// Group the batch positions by label, stable in draw order.
    fn group(&self, s: &mut Scratch) {
        s.m.iter_mut().for_each(|x| *x = 0);
        for &l in &s.labels {
            s.m[l as usize] += 1;
        }
        let mut run = 0u32;
        for (c, &m) in s.m.iter().enumerate() {
            s.offsets[c] = run;
            run += m;
        }
        s.offsets[self.k] = run;
        s.cursor.copy_from_slice(&s.offsets);
        for (pos, &l) in s.labels.iter().enumerate() {
            let slot = &mut s.cursor[l as usize];
            s.order[*slot as usize] = pos as u32;
            *slot += 1;
        }
    }

    /// Fold the grouped batch into the centroids. Returns the squared shift.
    fn fold(&self, centroids: &mut [f32], s: &mut Scratch) -> f64 {
        let dim = self.rows.dim;
        let window = self.cfg.ema_window.max(1);
        let mut shift = 0.0f64;
        for c in 0..self.k {
            let m = s.m[c];
            if m == 0 {
                s.streak[c] += 1;
                continue;
            }
            s.streak[c] = 0;
            s.acc.iter_mut().for_each(|x| *x = 0.0);
            let (lo, hi) = (s.offsets[c] as usize, s.offsets[c + 1] as usize);
            for &pos in &s.order[lo..hi] {
                let row = self.rows.row(s.idx[pos as usize] as usize);
                for (a, &v) in s.acc.iter_mut().zip(row) {
                    *a += f64::from(v);
                }
            }
            let eta = if self.plan.mode == Mode::Exact {
                1.0
            } else {
                s.counts[c] = (s.counts[c] + u64::from(m)).min(window);
                (f64::from(m) / s.counts[c] as f64)
                    .max(1.0 / window as f64)
                    .min(1.0)
            };
            let inv = 1.0 / f64::from(m);
            let cen = &mut centroids[c * dim..(c + 1) * dim];
            for (x, &a) in cen.iter_mut().zip(&s.acc) {
                let old = f64::from(*x);
                let new = old + eta * (a * inv - old);
                shift += (new - old) * (new - old);
                *x = new as f32;
            }
        }
        shift
    }

    /// Reseed centroids that stayed empty for `empty_patience` steps with the
    /// farthest batch points; lowest row index wins a tie.
    fn reseed_empty(&self, centroids: &mut [f32], s: &mut Scratch) {
        let patience = self.cfg.empty_patience.max(1);
        let is_empty = |s: &Scratch, c: usize| s.m[c] == 0 && s.streak[c] >= patience;
        if !(0..self.k).any(|c| is_empty(s, c)) {
            return;
        }
        let ranked = &mut s.order;
        ranked
            .iter_mut()
            .enumerate()
            .for_each(|(i, slot)| *slot = i as u32);
        let (idx, dists) = (&s.idx, &s.dists);
        ranked.sort_unstable_by(|&a, &b| {
            dists[b as usize]
                .total_cmp(&dists[a as usize])
                .then(idx[a as usize].cmp(&idx[b as usize]))
        });
        let dim = self.rows.dim;
        let mut next = 0usize;
        let mut last_row = u32::MAX;
        for c in 0..self.k {
            if !(s.m[c] == 0 && s.streak[c] >= patience) {
                continue;
            }
            while next < ranked.len() && idx[ranked[next] as usize] == last_row {
                next += 1;
            }
            let Some(&pos) = ranked.get(next) else { break };
            if dists[pos as usize] <= 0.0 {
                break;
            }
            last_row = idx[pos as usize];
            centroids[c * dim..(c + 1) * dim].copy_from_slice(self.rows.row(last_row as usize));
            s.streak[c] = 0;
            s.counts[c] = 1;
            next += 1;
        }
    }

    /// Run the steps. `run` is already past `Seeding`.
    pub(super) fn train(
        &self,
        run: &mut ClusterRun<'_>,
        centroids: &mut [f32],
    ) -> Result<(End, u32, f64), ClusterRefusal> {
        let mut s = Scratch::new(self.k, self.rows.dim, self.plan.batch);
        let mut rng = rng_for(self.cfg.seed ^ TRAIN_STREAM);
        let mut smooth: Option<f64> = None;
        let mut flat = 0u32;
        let mut step = 0u32;
        loop {
            self.check_cancel()?;
            self.draw(&mut s, &mut rng);
            run.advance(Event::BatchDrawn, step, 0.0)?;
            let (idx, labels, dists) = (&s.idx, &mut s.labels, &mut s.dists);
            self.assign_chunked(idx, centroids, labels, dists)?;
            let cur = s.dists.iter().map(|&d| f64::from(d)).sum::<f64>() / s.dists.len() as f64;
            self.group(&mut s);
            let shift = self.fold(centroids, &mut s);
            self.reseed_empty(centroids, &mut s);
            let (next, rel) = self.smooth_inertia(smooth, cur);
            smooth = Some(next);
            flat = if rel < self.cfg.tol { flat + 1 } else { 0 };
            step += 1;
            let exact = self.plan.mode == Mode::Exact;
            let settled = flat >= self.cfg.patience.max(1) && (exact || step >= self.cfg.min_steps);
            let converged = settled || (exact && shift == 0.0);
            let budget_left = step < self.plan.steps;
            run.advance(
                Event::StepDone {
                    converged,
                    budget_left,
                },
                step,
                rel,
            )?;
            if converged || !budget_left {
                let end = if converged {
                    End::Converged
                } else {
                    End::BudgetExhausted
                };
                return Ok((end, step, next));
            }
        }
    }

    /// The smoothed batch inertia and its relative change.
    fn smooth_inertia(&self, prev: Option<f64>, cur: f64) -> (f64, f64) {
        match prev {
            None => (cur, f64::INFINITY),
            Some(p) => {
                let next = if self.plan.mode == Mode::Exact {
                    cur
                } else {
                    0.8 * p + 0.2 * cur
                };
                let rel = (p - next).abs() / p.max(f64::MIN_POSITIVE);
                (next, if p == 0.0 && next == 0.0 { 0.0 } else { rel })
            }
        }
    }

    /// Label every row and total the inertia, in row order.
    pub(super) fn finish(&self, centroids: &[f32]) -> Result<(Vec<u32>, f64), ClusterRefusal> {
        let n = self.rows.n;
        let chunk = self.chunk_rows();
        let mut labels = vec![0u32; n];
        let mut total = 0.0f64;
        let mut idx = Vec::with_capacity(chunk);
        let mut dists = vec![0.0f32; chunk];
        for start in (0..n).step_by(chunk) {
            self.check_cancel()?;
            let end = (start + chunk).min(n);
            idx.clear();
            idx.extend(start as u32..end as u32);
            let len = end - start;
            self.backend.assign(
                self.rows,
                &idx,
                centroids,
                self.k,
                &mut labels[start..end],
                &mut dists[..len],
            );
            total += dists[..len].iter().map(|&d| f64::from(d)).sum::<f64>();
        }
        Ok((labels, total))
    }
}

/// The backend of a run (the registered one, else the CPU path).
pub(super) fn backend() -> std::sync::Arc<dyn AssignBackend> {
    active_backend()
}

/// A strided probe used by tests: the first `count` seeded indexes.
#[cfg(test)]
pub(super) fn sample_for_test(seed: u64, n: usize, count: usize) -> Vec<u32> {
    super::seed::strided_sample(&mut rng_for(seed), n, count)
}

#[cfg(test)]
mod tests {
    use super::super::{fit_simple, ClusterConfig, Rows};
    use super::*;

    /// One step on one centroid: `c = 0 + eta * (batch_mean - 0)` with
    /// `eta = m / min(1 + m, window)`, folded over the draws in order.
    // spec: MEDULLA-CLUSTER-R002.3
    #[test]
    fn moving_average_matches_reference_fold() {
        let data: Vec<f32> = (0..10).map(|i| i as f32 + 1.0).collect();
        let cfg = ClusterConfig {
            seed: 5,
            batch_rows: 4,
            min_steps: 1,
            max_steps: 1,
            warm_start: Some(vec![0.0]),
            labels: false,
            threads: 1,
            ..ClusterConfig::default()
        };
        let mut rng = rng_for(cfg.seed ^ TRAIN_STREAM);
        let drawn: Vec<f32> = (0..4).map(|_| data[rng.gen_range(0..10usize)]).collect();
        let mean = drawn.iter().map(|&v| f64::from(v)).sum::<f64>() / 4.0;
        let expected = (0.0 + (4.0 / 5.0) * (mean - 0.0)) as f32;
        let out = fit_simple(Rows::new(&data, 10, 1), 1, &cfg).unwrap();
        assert_eq!(out.centroids[0].to_bits(), expected.to_bits());
        assert_eq!(out.end, End::BudgetExhausted);
    }

    #[test]
    fn strided_sample_is_distinct_and_ascending() {
        let s = sample_for_test(3, 1000, 100);
        assert!(s.windows(2).all(|w| w[0] < w[1]));
    }
}
