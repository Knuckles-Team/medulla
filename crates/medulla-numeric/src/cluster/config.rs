//! Clustering configuration and the default of every knob (MEDULLA-CLUSTER-R002.5).
//!
//! The knob table lives in `medulla-decision-stats`; the defaults live here so the
//! kernel needs no decision types. A test in that crate asserts the two agree.

/// Default seed of every run that does not name one.
pub const DEFAULT_SEED: u64 = 42;
pub const DEFAULT_BATCH_ROWS: usize = 0;
pub const DEFAULT_BATCH_MAX_ROWS: usize = 65_536;
pub const DEFAULT_EPOCHS: f64 = 3.0;
pub const DEFAULT_STEP_BUDGET_MADDS: u64 = 2_147_483_648;
pub const DEFAULT_MIN_STEPS: u32 = 8;
pub const DEFAULT_MAX_STEPS: u32 = 300;
pub const DEFAULT_TOL: f64 = 0.001;
pub const DEFAULT_PATIENCE: u32 = 3;
pub const DEFAULT_EMPTY_PATIENCE: u32 = 2;
pub const DEFAULT_EMA_WINDOW: u64 = 8_192;
pub const DEFAULT_PLUSPLUS_MAX_K: usize = 4_096;
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 1_073_741_824;
pub const DEFAULT_THREADS: usize = 0;
pub const DEFAULT_REFRESH_DRIFT_RATIO: f64 = 1.5;
pub const DEFAULT_ONLINE_WINDOW: u64 = 4_096;
pub const DEFAULT_SNAPSHOT_CHUNK_ROWS: usize = 4_096;

/// Everything one run reads. `Default` holds the knob defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterConfig {
    pub seed: u64,
    /// Force exact (full-batch) mode.
    pub exact: bool,
    /// Clamp `k` to what the input supports instead of refusing.
    pub at_most_k: bool,
    /// Compute the label of every row and the exact inertia after training.
    pub labels: bool,
    /// `k * dim` seeds that replace seeding (online retrain).
    pub warm_start: Option<Vec<f32>>,
    pub batch_rows: usize,
    pub batch_max_rows: usize,
    pub epochs: f64,
    pub step_budget_madds: u64,
    pub min_steps: u32,
    pub max_steps: u32,
    pub tol: f64,
    pub patience: u32,
    pub empty_patience: u32,
    pub ema_window: u64,
    pub plusplus_max_k: usize,
    pub memory_budget_bytes: u64,
    pub threads: usize,
    pub refresh_drift_ratio: f64,
    pub online_window: u64,
    pub snapshot_chunk_rows: usize,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            exact: false,
            at_most_k: false,
            labels: true,
            warm_start: None,
            batch_rows: DEFAULT_BATCH_ROWS,
            batch_max_rows: DEFAULT_BATCH_MAX_ROWS,
            epochs: DEFAULT_EPOCHS,
            step_budget_madds: DEFAULT_STEP_BUDGET_MADDS,
            min_steps: DEFAULT_MIN_STEPS,
            max_steps: DEFAULT_MAX_STEPS,
            tol: DEFAULT_TOL,
            patience: DEFAULT_PATIENCE,
            empty_patience: DEFAULT_EMPTY_PATIENCE,
            ema_window: DEFAULT_EMA_WINDOW,
            plusplus_max_k: DEFAULT_PLUSPLUS_MAX_K,
            memory_budget_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            threads: DEFAULT_THREADS,
            refresh_drift_ratio: DEFAULT_REFRESH_DRIFT_RATIO,
            online_window: DEFAULT_ONLINE_WINDOW,
            snapshot_chunk_rows: DEFAULT_SNAPSHOT_CHUNK_ROWS,
        }
    }
}

impl ClusterConfig {
    /// The pool size: the knob, or half the cores (at least one) when it is zero.
    pub fn resolved_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads;
        }
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        (cores / 2).max(1)
    }
}
