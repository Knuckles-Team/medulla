//! The one clustering kernel (MEDULLA-CLUSTER-R001): seeded mini-batch updates with
//! moving-average centroids (e-means), the exact full-batch mode, a run state
//! machine, typed refusals, a memory bound and online centroid tracking.
//!
//! Every caller that trains centroids in the engine calls [`fit`] or
//! [`seed_centers`]. The surface-B `kmeans(vec_col, k)` aggregate, the Python kernel and
//! the served data-science method all enter here.

mod backend;
mod config;
mod distance;
mod online;
mod plan;
mod refusal;
mod seed;
mod state;
mod train;

pub use backend::{
    active_backend, clear_assign_backend, set_assign_backend, AssignBackend, CpuBackend, Rows,
};
pub use config::*;
pub use distance::{lane_reduce, nearest, nearest_with_dist, sq_dist};
pub use online::CentroidTracker;
pub use plan::{plan, training_bytes, Mode, Plan};
pub use refusal::ClusterRefusal;
pub use seed::{seed_centers, seed_indices};
pub use state::{
    transition, CancelToken, ClusterRun, Event, NoProgress, Progress, ProgressSink, State,
};
pub use train::{ClusterOutcome, End};

use crate::error::Result;
use ndarray::{Array2, ArrayView2};
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Everything `fit` needs beyond the rows.
pub struct FitArgs<'a> {
    pub rows: Rows<'a>,
    pub k: usize,
    pub cfg: &'a ClusterConfig,
    pub sink: &'a dyn ProgressSink,
    pub cancel: &'a CancelToken,
}

static POOLS: Mutex<Option<HashMap<usize, Arc<ThreadPool>>>> = Mutex::new(None);

/// The kernel's own compute pool of `threads` threads (one per distinct size).
fn pool_for(threads: usize) -> std::result::Result<Arc<ThreadPool>, ClusterRefusal> {
    let mut guard = POOLS.lock().map_err(|_| pool_failure())?;
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(pool) = map.get(&threads) {
        return Ok(pool.clone());
    }
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("cluster-kernel-{i}"))
        .build()
        .map_err(|_| pool_failure())?;
    let pool = Arc::new(pool);
    map.insert(threads, pool.clone());
    Ok(pool)
}

fn pool_failure() -> ClusterRefusal {
    ClusterRefusal::ShapeInvalid("the compute pool could not start")
}

/// Shape, `k`, finiteness and distinct-row checks. Returns the usable `k`.
fn validate(
    rows: Rows<'_>,
    k: usize,
    cfg: &ClusterConfig,
) -> std::result::Result<usize, ClusterRefusal> {
    use rayon::prelude::*;
    let (n, dim) = (rows.n, rows.dim);
    if n == 0 || dim == 0 {
        return Err(ClusterRefusal::ShapeInvalid("empty input or zero width"));
    }
    if rows.data.len() != n * dim {
        return Err(ClusterRefusal::ShapeInvalid("rows do not match n * dim"));
    }
    if n > u32::MAX as usize {
        return Err(ClusterRefusal::ShapeInvalid("more than 2^32 - 1 rows"));
    }
    if k == 0 {
        return Err(ClusterRefusal::KInvalid);
    }
    let k = if k > n {
        if !cfg.at_most_k {
            return Err(ClusterRefusal::KExceedsRows { k, n });
        }
        n
    } else {
        k
    };
    let bad = rows
        .data
        .par_chunks(dim)
        .position_first(|r| r.iter().any(|v| !v.is_finite()));
    if let Some(row) = bad {
        let col = rows
            .row(row)
            .iter()
            .position(|v| !v.is_finite())
            .unwrap_or(0);
        return Err(ClusterRefusal::InputNotFinite { row, col });
    }
    let distinct = distinct_up_to(rows, k);
    if distinct < k {
        if !cfg.at_most_k {
            return Err(ClusterRefusal::TooFewDistinct { k });
        }
        return Ok(distinct);
    }
    Ok(k)
}

/// Count distinct rows, stopping at `cap`. A fixed open-addressing table of
/// `next_power_of_two(2 * cap)` words keeps the memory at most `32 * cap` bytes.
fn distinct_up_to(rows: Rows<'_>, cap: usize) -> usize {
    let size = (2 * cap).next_power_of_two().max(2);
    let mut table = vec![0u64; size];
    let mask = size - 1;
    let mut count = 0usize;
    for i in 0..rows.n {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in rows.row(i) {
            h = (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3);
            h ^= h >> 29;
        }
        let h = h | 1;
        let mut slot = (h >> 1) as usize & mask;
        loop {
            if table[slot] == 0 {
                table[slot] = h;
                count += 1;
                break;
            }
            if table[slot] == h {
                break;
            }
            slot = (slot + 1) & mask;
        }
        if count >= cap {
            return count;
        }
    }
    count
}

/// Train `k` centroids over `args.rows`. Refuses before work on bad input or an
/// over-budget plan, and ends within the plan's step budget.
pub fn fit(args: FitArgs<'_>) -> std::result::Result<ClusterOutcome, ClusterRefusal> {
    let cfg = args.cfg;
    let max_steps = cfg.max_steps.max(1);
    let mut run = ClusterRun::new(args.sink, max_steps);
    match fit_inner(&args, &mut run) {
        Ok(outcome) => Ok(outcome),
        Err(refusal) => Err(run.refuse(refusal)),
    }
}

fn fit_inner(
    args: &FitArgs<'_>,
    run: &mut ClusterRun<'_>,
) -> std::result::Result<ClusterOutcome, ClusterRefusal> {
    let (rows, cfg) = (args.rows, args.cfg);
    if rows.n == 0 || rows.dim == 0 {
        return Err(ClusterRefusal::ShapeInvalid("empty input or zero width"));
    }
    let k_wanted = if cfg.at_most_k {
        args.k.min(rows.n)
    } else {
        args.k
    };
    let plan = plan::plan(rows.n, k_wanted.max(1), rows.dim, cfg);
    let need = training_bytes(k_wanted, rows.dim, plan.batch);
    if need > cfg.memory_budget_bytes {
        return Err(ClusterRefusal::MemoryBudget {
            need,
            budget: cfg.memory_budget_bytes,
        });
    }
    let k = validate(rows, args.k, cfg)?;
    let plan = plan::plan(rows.n, k, rows.dim, cfg);
    if args.cancel.is_cancelled() {
        return Err(ClusterRefusal::Cancelled);
    }
    let pool = pool_for(cfg.resolved_threads())?;
    pool.install(|| run_on_pool(args, run, k, plan))
}

fn run_on_pool(
    args: &FitArgs<'_>,
    run: &mut ClusterRun<'_>,
    k: usize,
    plan: Plan,
) -> std::result::Result<ClusterOutcome, ClusterRefusal> {
    let (rows, cfg) = (args.rows, args.cfg);
    let mut centroids = match &cfg.warm_start {
        Some(w) if w.len() == k * rows.dim => w.clone(),
        Some(_) => {
            return Err(ClusterRefusal::ShapeInvalid(
                "warm start does not match k * dim",
            ))
        }
        None => seed_centers(rows, k, cfg.seed, cfg)?,
    };
    run.advance(Event::SeedsReady, 0, 0.0)?;
    let trainer = train::Trainer {
        rows,
        k,
        cfg,
        plan,
        cancel: args.cancel,
        backend: train::backend(),
    };
    let (end, steps, smooth) = trainer.train(run, &mut centroids)?;
    let n = rows.n as f64;
    if cfg.labels {
        let (labels, inertia) = trainer.finish(&centroids)?;
        return Ok(ClusterOutcome {
            centroids,
            labels,
            end,
            steps,
            inertia,
            mean_sq_dist: inertia / n,
        });
    }
    Ok(ClusterOutcome {
        centroids,
        labels: Vec::new(),
        end,
        steps,
        inertia: smooth * n,
        mean_sq_dist: smooth,
    })
}

/// Run `f` on the kernel's pool of `threads` threads. Work that is not clustering
/// but belongs to the same build (rotation maps, encoding) uses it so a build stays
/// within `cluster.threads` cores.
pub fn run_in_pool<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    match pool_for(threads.max(1)) {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}

/// `fit` with no progress sink and no cancellation.
pub fn fit_simple(
    rows: Rows<'_>,
    k: usize,
    cfg: &ClusterConfig,
) -> std::result::Result<ClusterOutcome, ClusterRefusal> {
    let cancel = CancelToken::new();
    fit(FitArgs {
        rows,
        k,
        cfg,
        sink: &NoProgress,
        cancel: &cancel,
    })
}

// ---- the ndarray surface (SQL aggregate, Python kernel) ----

/// The result of a [`kmeans`] fit: a hard label per input row plus the fitted centroids
/// and the final inertia (within-cluster sum of squared distances).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract", derive(serde::Serialize, serde::Deserialize))]
pub struct KMeansResult {
    /// Cluster index (`0..k`) for each input row, in input order.
    pub labels: Vec<usize>,
    /// The `k×d` centroid matrix (one centroid per row).
    pub centroids: Array2<f64>,
    /// Within-cluster sum of squared distances at the end of the run.
    pub inertia: f64,
    /// Number of steps actually run (at most `max_iter`).
    pub n_iter: usize,
}

/// The default RNG seed of the `kmeans` aggregate, so in-engine clustering is
/// reproducible without the SQL caller passing one.
pub const KMEANS_DEFAULT_SEED: u64 = DEFAULT_SEED;

/// Cluster `data` (`n×d`) into `k` groups with the kernel. `max_iter` is the step
/// budget; `k > n` clamps to `n`. Empty data and `k = 0` refuse.
pub fn kmeans(data: ArrayView2<f64>, k: usize, max_iter: usize, seed: u64) -> Result<KMeansResult> {
    let (n, d) = data.dim();
    let flat: Vec<f32> = data.iter().map(|&v| v as f32).collect();
    let cfg = ClusterConfig {
        seed,
        at_most_k: true,
        max_steps: u32::try_from(max_iter.max(1)).unwrap_or(u32::MAX),
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&flat, n, d), k, &cfg)?;
    let kk = out.centroids.len() / d.max(1);
    let centroids = Array2::from_shape_fn((kk, d), |(c, j)| f64::from(out.centroids[c * d + j]));
    Ok(KMeansResult {
        labels: out.labels.iter().map(|&l| l as usize).collect(),
        centroids,
        inertia: out.inertia,
        n_iter: out.steps as usize,
    })
}

/// The cluster labels only — the shape the `kmeans(vec_col, k)` aggregate returns.
pub fn kmeans_labels(data: ArrayView2<f64>, k: usize) -> Result<Vec<usize>> {
    Ok(kmeans(data, k, 100, KMEANS_DEFAULT_SEED)?.labels)
}
