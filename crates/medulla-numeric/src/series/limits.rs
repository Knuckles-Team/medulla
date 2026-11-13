//! The fixed safety ceilings of the state-space series path (R002.4).
//!
//! Each constant is the single definition; the derived-series store and the served
//! handlers import them. Every check runs before the allocation or the work it guards
//! and returns the stable code the wire carries. A model revision may lower a ceiling
//! and never raise it (the checks take the ceiling as an argument for that reason).

/// Largest state dimension of a model.
pub const MAX_STATE_DIM: usize = 16;
/// Largest observation (measurement) dimension of a model.
pub const MAX_OBS_DIM: usize = 8;
/// Most observations one maintenance step retains (consumes) at once.
pub const MAX_RETAINED_OBS: usize = 4096;
/// Largest encoded derived state (checkpoint bytes) a series may keep.
pub const MAX_CHECKPOINT_BYTES: usize = 1 << 20;
/// Most multiply-adds one filter update may cost.
pub const MAX_UPDATE_WORK: u64 = 1_000_000;
/// Most source points one replay or work step reads.
pub const MAX_REPLAY_POINTS: usize = 100_000;
/// Most observations a regime-HMM refit takes (the one definition lives with the wire type).
pub const MAX_HMM_REFIT_OBS: usize = medulla_types::numeric_kernel::MAX_REFIT_OBSERVATIONS;

/// Which ceiling a check refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceiling {
    StateDim,
    ObsDim,
    RetainedObs,
    CheckpointBytes,
    UpdateWork,
    ReplayPoints,
    HmmRefit,
}

/// A refused ceiling: its stable code and what overflowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitRefusal {
    pub ceiling: Ceiling,
    pub requested: u64,
    pub limit: u64,
}

impl LimitRefusal {
    /// `KERNEL_STATE_BUDGET` for state and refit overflow, `KERNEL_WORK_BUDGET` for work and
    /// replay overflow.
    pub fn code(&self) -> &'static str {
        match self.ceiling {
            Ceiling::StateDim
            | Ceiling::ObsDim
            | Ceiling::RetainedObs
            | Ceiling::CheckpointBytes
            | Ceiling::HmmRefit => "KERNEL_STATE_BUDGET",
            Ceiling::UpdateWork | Ceiling::ReplayPoints => "KERNEL_WORK_BUDGET",
        }
    }
}

impl std::fmt::Display for LimitRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {:?} is {} (consumed), over the ceiling of {}",
            self.code(),
            self.ceiling,
            self.requested,
            self.limit
        )
    }
}

fn within(ceiling: Ceiling, requested: u64, limit: u64) -> Result<(), LimitRefusal> {
    if requested > limit {
        Err(LimitRefusal {
            ceiling,
            requested,
            limit,
        })
    } else {
        Ok(())
    }
}

/// Multiply-adds of one predict-update step over `n` states and `m` measurements.
pub fn update_work(n: usize, m: usize) -> u64 {
    let (n, m) = (n as u64, m as u64);
    2 * n * n * n + 2 * n * n * m + 2 * n * m * m + m * m * m
}

/// State dimension within `limit` (never above [`MAX_STATE_DIM`]).
pub fn check_state_dim(n: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(Ceiling::StateDim, n as u64, limit.min(MAX_STATE_DIM) as u64)
}

/// Observation dimension within `limit` (never above [`MAX_OBS_DIM`]).
pub fn check_obs_dim(m: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(Ceiling::ObsDim, m as u64, limit.min(MAX_OBS_DIM) as u64)
}

/// Retained observations within `limit` (never above [`MAX_RETAINED_OBS`]).
pub fn check_retained(count: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(
        Ceiling::RetainedObs,
        count as u64,
        limit.min(MAX_RETAINED_OBS) as u64,
    )
}

/// Encoded checkpoint bytes within `limit` (never above [`MAX_CHECKPOINT_BYTES`]).
pub fn check_checkpoint_bytes(bytes: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(
        Ceiling::CheckpointBytes,
        bytes as u64,
        limit.min(MAX_CHECKPOINT_BYTES) as u64,
    )
}

/// One update's multiply-adds within `limit` (never above [`MAX_UPDATE_WORK`]).
pub fn check_update_work(work: u64, limit: u64) -> Result<(), LimitRefusal> {
    within(Ceiling::UpdateWork, work, limit.min(MAX_UPDATE_WORK))
}

/// Replay points within `limit` (never above [`MAX_REPLAY_POINTS`]).
pub fn check_replay_points(points: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(
        Ceiling::ReplayPoints,
        points as u64,
        limit.min(MAX_REPLAY_POINTS) as u64,
    )
}

/// HMM refit observations within `limit` (never above [`MAX_HMM_REFIT_OBS`]).
pub fn check_hmm_refit(observations: usize, limit: usize) -> Result<(), LimitRefusal> {
    within(
        Ceiling::HmmRefit,
        observations as u64,
        limit.min(MAX_HMM_REFIT_OBS) as u64,
    )
}
