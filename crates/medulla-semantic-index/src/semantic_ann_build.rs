//! Index build that holds no store lock (MEDULLA-CLUSTER-R011.1).
//!
//! The old build held the index write lock for its whole run, and its caller held
//! the store read lock beside it, so a writer or a query waited for minutes. A build
//! now copies bounded blocks under short read holds, trains and encodes with no lock,
//! records the ids that writers touch meanwhile, and installs the index under one
//! short hold that replays that journal.

use super::*;
use crate::semantic_ann::{nlist_for, normalize, MAX_INDEX_ROWS};
use medulla_numeric::cluster::{self, CancelToken, ClusterConfig, NoProgress, ProgressSink};
use std::collections::HashSet;
use std::sync::Arc;

/// Cancellation, progress and kernel settings of one build.
#[derive(Clone)]
pub struct BuildHooks {
    pub cluster: ClusterConfig,
    pub cancel: CancelToken,
    pub sink: Arc<dyn ProgressSink>,
}

impl Default for BuildHooks {
    fn default() -> Self {
        Self {
            cluster: ClusterConfig::default(),
            cancel: CancelToken::new(),
            sink: Arc::new(NoProgress),
        }
    }
}

/// How a build ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    /// The index is installed.
    Built,
    /// Nothing to do: too few vectors, a build already running, or an unsupported width.
    Skipped,
    /// The cancel token fired; nothing was installed.
    Cancelled,
    /// The build refused or failed; nothing was installed.
    Failed(String),
}

/// What `begin_build` reports to the driver.
pub(super) struct BuildStart {
    pub n: usize,
    pub dim: usize,
}

impl SemanticStore {
    /// An in-place build for callers that already hold the store (`warm`, export).
    pub(super) fn build_in_place(&self, label: &str) {
        let Some(start) = self.begin_build(false) else {
            return;
        };
        let hooks = BuildHooks::default();
        match run_build(&|| self, &start, &hooks) {
            Ok(ann) => {
                if self.finish_build(ann) {
                    #[cfg(test)]
                    self.builds.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(
                        graph = label,
                        n_vectors = start.n,
                        "semantic ANN index build complete"
                    );
                }
            }
            Err(_) => self.abort_build(),
        }
    }
}

impl SemanticStore {
    /// Claim a build and start the write journal. `rebuild` claims from `READY`
    /// (serving continues on the old index), else from `COLD`.
    pub(super) fn begin_build(&self, rebuild: bool) -> Option<BuildStart> {
        let (n, dim) = (self.arena.len(), self.arena.dim);
        if n < BRUTE_FORCE_THRESHOLD.max(ANN_BUILD_THRESHOLD)
            || n > MAX_INDEX_ROWS
            || dim > MAX_MAINTAINED_DIMENSION
        {
            return None;
        }
        let (from, to) = if rebuild {
            (STATE_READY, STATE_REBUILDING)
        } else {
            (STATE_COLD, STATE_WARMING)
        };
        self.state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        *self.build_journal.lock() = Some(HashSet::new());
        Some(BuildStart { n, dim })
    }

    /// Drop the journal and release the claim without installing anything.
    pub(super) fn abort_build(&self) {
        *self.build_journal.lock() = None;
        let back = if self.index.read().is_some() {
            STATE_READY
        } else {
            STATE_COLD
        };
        self.state.store(back, Ordering::Release);
    }

    /// Up to `count` training rows at positions `first*stride, (first+1)*stride, ...`,
    /// normalized. Rows that moved since the build began are still valid training data.
    pub(super) fn sample_block(&self, first: usize, count: usize, stride: usize) -> Vec<Vec<f32>> {
        (first..first + count)
            .map(|k| k * stride)
            .take_while(|&p| p < self.arena.len())
            .map(|p| normalize(self.arena.row(p)))
            .collect()
    }

    /// Normalized rows `[from, from + count)` with their ids, and the arena length now.
    pub(super) fn snapshot_chunk(
        &self,
        from: usize,
        count: usize,
    ) -> (Vec<(String, Vec<f32>)>, usize) {
        let len = self.arena.len();
        let end = (from + count).min(len);
        let rows = (from.min(end)..end)
            .map(|r| (self.arena.ids[r].clone(), normalize(self.arena.row(r))))
            .collect();
        (rows, len)
    }

    /// Record the ids a writer touched while a build runs: the written id, and for a
    /// removal also the id of the row that moves into the vacated slot.
    pub(super) fn note_build_write(&mut self, ids: &[&str]) {
        if let Some(journal) = self.build_journal.get_mut().as_mut() {
            journal.extend(ids.iter().map(|s| (*s).to_string()));
        }
    }

    /// Install `ann`: replay the journal against the current arena, then swap.
    /// The caller holds the store read lock, so no writer runs meanwhile.
    pub(super) fn finish_build(&self, mut ann: AnnIndex) -> bool {
        let touched = self.build_journal.lock().take().unwrap_or_default();
        for id in &touched {
            match self.arena.get(id) {
                Some(emb) => {
                    ann.add(id, emb);
                }
                None => {
                    ann.remove(id);
                }
            }
        }
        if ann.live_len() != self.arena.len() {
            tracing::warn!(
                indexed = ann.live_len(),
                resident = self.arena.len(),
                "semantic ANN build disagrees with the resident arena; discarded"
            );
            self.abort_build();
            return false;
        }
        let n = self.arena.len();
        *self.index.write() = Some(ann);
        *self.built_len.write() = n;
        self.state.store(STATE_READY, Ordering::Release);
        true
    }

    /// Whether the resident index has drifted past its refresh threshold.
    pub fn ann_stale(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_READY
            && self
                .index
                .try_read()
                .is_some_and(|g| g.as_ref().is_some_and(AnnIndex::is_stale))
    }

    /// The resident index's drift ratio (ingest spread over training spread), once the
    /// tracker has seen enough rows; `None` without an index or a tracker.
    pub fn ann_drift_ratio(&self) -> Option<f64> {
        self.index
            .try_read()
            .and_then(|g| g.as_ref().and_then(AnnIndex::drift_ratio))
    }

    /// True while a rebuild runs and the old index still serves.
    pub fn is_rebuilding(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_REBUILDING
    }
}

/// Training sample block size and stride for `n` rows.
fn sample_shape(n: usize) -> (usize, usize) {
    let nlist = nlist_for(n);
    let target = (nlist * 40).clamp(1, n);
    (target, (n / target).max(1))
}

/// Build the index of `lock`'s store without holding the lock across the work.
///
/// Each step that reads the store takes `lock.read()` for at most
/// `cluster.snapshot_chunk_rows` rows. Training and encoding hold no lock. The
/// install holds the read lock for the journal replay only.
pub fn build_off_lock(
    lock: &RwLock<SemanticStore>,
    label: &str,
    rebuild: bool,
    hooks: &BuildHooks,
) -> BuildOutcome {
    let Some(start) = lock.read().begin_build(rebuild) else {
        return BuildOutcome::Skipped;
    };
    let started = std::time::Instant::now();
    let result = run_build(&|| lock.read(), &start, hooks);
    match result {
        Ok(ann) => {
            if lock.read().finish_build(ann) {
                tracing::info!(
                    graph = label,
                    n_vectors = start.n,
                    build_ms = started.elapsed().as_millis() as u64,
                    "semantic ANN index built off the store lock"
                );
                BuildOutcome::Built
            } else {
                BuildOutcome::Failed("the built index disagreed with the resident rows".into())
            }
        }
        Err(outcome) => {
            lock.read().abort_build();
            outcome
        }
    }
}

/// The build steps. `acquire` takes the store read hold for one bounded read: the
/// real driver passes the store lock; an in-place build passes the store itself.
pub(super) fn run_build<G: std::ops::Deref<Target = SemanticStore>>(
    acquire: &dyn Fn() -> G,
    start: &BuildStart,
    hooks: &BuildHooks,
) -> Result<AnnIndex, BuildOutcome> {
    let chunk = hooks.cluster.snapshot_chunk_rows.max(1);
    let (target, stride) = sample_shape(start.n);
    let mut sample = Vec::with_capacity(target);
    let mut first = 0;
    while first < target {
        check_cancel(hooks)?;
        sample.extend(acquire().sample_block(first, chunk, stride));
        first += chunk;
    }
    if sample.is_empty() {
        return Err(BuildOutcome::Skipped);
    }
    let trained =
        AnnIndex::train_codebooks(&sample, start.dim, start.n, hooks).map_err(refusal_outcome)?;
    let mut rows: Vec<(String, Vec<f32>)> = Vec::with_capacity(start.n);
    let mut from = 0;
    loop {
        check_cancel(hooks)?;
        let (block, len) = acquire().snapshot_chunk(from, chunk);
        from += block.len().max(1);
        rows.extend(block);
        if from >= len {
            break;
        }
    }
    check_cancel(hooks)?;
    AnnIndex::from_trained(trained, &sample, rows, start.dim, hooks)
        .ok_or_else(|| BuildOutcome::Failed("the snapshot did not form a valid index".into()))
}

fn check_cancel(hooks: &BuildHooks) -> Result<(), BuildOutcome> {
    if hooks.cancel.is_cancelled() {
        Err(BuildOutcome::Cancelled)
    } else {
        Ok(())
    }
}

fn refusal_outcome(refusal: cluster::ClusterRefusal) -> BuildOutcome {
    if refusal == cluster::ClusterRefusal::Cancelled {
        BuildOutcome::Cancelled
    } else {
        BuildOutcome::Failed(refusal.to_string())
    }
}
