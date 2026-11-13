//! The matrix Kalman recurrence behind the served state-space series
//! (MEDULLA-FINANCE-PRIMITIVES-R002.6 to R002.8), re-exported from `series::kalman`.
//!
//! One causal predict-update recurrence ([`StateSpaceFilter::update`]) serves the four
//! profiles (volatility, mean reversion, hedge ratio, factor sensitivity); a bounded
//! innovation-shift detector retunes the measurement noise within fixed bounds; a robust
//! extended update ([`StateSpaceFilter::update_robust`]) bounds tail influence for the
//! registered nonlinear profiles. The scalar [`super::kalman::Kalman`] stays the scalar
//! fast path behind the `kalman` / `kbeta` series functions and is not touched.
//!
//! Every operation computes into locals and commits at the end: a refusal leaves the
//! filter exactly as it was.

use serde::{Deserialize, Serialize};

use crate::detkernel::math::{exp, ln};

/// Symmetry tolerance a model revision pins by default.
pub const SYMMETRY_TOL: f64 = 1e-9;
/// Decomposition (pivot) tolerance a model revision pins by default.
pub const DECOMPOSITION_TOL: f64 = 1e-12;
/// Positive floor applied to a squared return before the logarithm.
pub const VOLATILITY_FLOOR: f64 = 1e-12;
/// `-E[ln chi2(1)]`: the bias a log-squared return carries.
pub const LOG_CHI2_BIAS: f64 = 1.2704;
/// Observations the volatility band classes wait for.
pub const VOLATILITY_WARMUP: usize = 60;
/// Latent variance below this multiple of the warmup median is `Low`.
pub const VOLATILITY_LOW_RATIO: f64 = 0.5;
/// Latent variance above this multiple of the warmup median is `High`.
pub const VOLATILITY_HIGH_RATIO: f64 = 2.0;
/// The Huber-style cap on the standardized innovation distance of the robust update.
pub const ROBUST_CAP: f64 = 2.5;
/// Each whitened innovation component is clipped to this magnitude for shift detection.
pub const WHITENED_CLIP: f64 = 8.0;

/// A typed refusal of the state-space kernels; every one leaves the filter unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterRefusal {
    Singular,
    ParamInvalid,
    InputNonfinite,
    DimensionMismatch,
    NoiseCorrelated,
    TransformInvalid,
    UnitMismatch,
    FactorOrder,
    FutureLeak,
    Warmup,
    RetuneBudget,
    WindowInvalid,
    ModelUnregistered,
    JacobianInvalid,
    DomainViolation,
}

impl FilterRefusal {
    /// The registered wire token (`ServerErrorCode` / `KernelCode`).
    pub fn code(self) -> &'static str {
        match self {
            Self::Singular => "KERNEL_SINGULAR",
            Self::ParamInvalid => "KERNEL_PARAM_INVALID",
            Self::InputNonfinite => "KERNEL_INPUT_NONFINITE",
            Self::DimensionMismatch => "STATE_DIMENSION_MISMATCH",
            Self::NoiseCorrelated => "STATE_NOISE_CORRELATED",
            Self::TransformInvalid => "STATE_TRANSFORM_INVALID",
            Self::UnitMismatch => "STATE_UNIT_MISMATCH",
            Self::FactorOrder => "STATE_FACTOR_ORDER",
            Self::FutureLeak => "STATE_FUTURE_LEAK",
            Self::Warmup => "STATE_WARMUP",
            Self::RetuneBudget => "STATE_RETUNE_BUDGET",
            Self::WindowInvalid => "STATE_WINDOW_INVALID",
            Self::ModelUnregistered => "STATE_MODEL_UNREGISTERED",
            Self::JacobianInvalid => "STATE_JACOBIAN_INVALID",
            Self::DomainViolation => "STATE_DOMAIN_VIOLATION",
        }
    }
}

type Res<T> = Result<T, FilterRefusal>;

// ---------------------------------------------------------------- dense helpers (row-major)

fn matmul(a: &[f64], ar: usize, ac: usize, b: &[f64], bc: usize) -> Vec<f64> {
    let mut out = vec![0.0; ar * bc];
    for i in 0..ar {
        for k in 0..ac {
            let aik = a[i * ac + k];
            for j in 0..bc {
                out[i * bc + j] += aik * b[k * bc + j];
            }
        }
    }
    out
}

fn transpose(a: &[f64], r: usize, c: usize) -> Vec<f64> {
    let mut out = vec![0.0; r * c];
    for i in 0..r {
        for j in 0..c {
            out[j * r + i] = a[i * c + j];
        }
    }
    out
}

fn symmetrize(a: &mut [f64], n: usize) {
    for i in 0..n {
        for j in 0..i {
            let m = 0.5 * (a[i * n + j] + a[j * n + i]);
            a[i * n + j] = m;
            a[j * n + i] = m;
        }
    }
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Lower Cholesky factor of a symmetric positive-definite `s`; `None` on a pivot at or
/// below `tol` (singular or indefinite).
fn cholesky(s: &[f64], n: usize, tol: f64) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = s[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if !(sum > tol) {
                    return None;
                }
                l[i * n + i] = sum.sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }
    Some(l)
}

/// Solve `L y = b` (`b` is `n x c`).
fn forward(l: &[f64], n: usize, b: &[f64], c: usize) -> Vec<f64> {
    let mut y = b.to_vec();
    for col in 0..c {
        for i in 0..n {
            let mut sum = y[i * c + col];
            for k in 0..i {
                sum -= l[i * n + k] * y[k * c + col];
            }
            y[i * c + col] = sum / l[i * n + i];
        }
    }
    y
}

/// Solve `Lᵀ x = y` (`y` is `n x c`).
fn backward(l: &[f64], n: usize, y: &[f64], c: usize) -> Vec<f64> {
    let mut x = y.to_vec();
    for col in 0..c {
        for i in (0..n).rev() {
            let mut sum = x[i * c + col];
            for k in i + 1..n {
                sum -= l[k * n + i] * x[k * c + col];
            }
            x[i * c + col] = sum / l[i * n + i];
        }
    }
    x
}

/// A deterministic 64-bit fingerprint (FNV-1a over the bit patterns) of a parameter set.
fn fingerprint(values: &[f64]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for v in values {
        for byte in v.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("fnv1a64:{hash:016x}")
}

// ---------------------------------------------------------------- shift-detector state machine

/// The phases of the innovation-shift detector (explicit machine `innovation_shift`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShiftPhase {
    /// Filling the baseline window.
    Seeding,
    /// Comparing the recent window with the baseline.
    Armed,
    /// Waiting out the cooldown after a retune.
    Cooldown,
}

/// The closed event set of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftEvent {
    BaselineFilled,
    ShiftConfirmed,
    CooldownElapsed,
}

/// An event refused in the current phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShiftTransitionError {
    pub machine: &'static str,
    pub state: ShiftPhase,
    pub event: ShiftEvent,
}

impl ShiftPhase {
    /// The one transition function: the only writer of the phase.
    pub fn transition(self, event: ShiftEvent) -> Result<ShiftPhase, ShiftTransitionError> {
        match (self, event) {
            (Self::Seeding, ShiftEvent::BaselineFilled) => Ok(Self::Armed),
            (Self::Armed, ShiftEvent::ShiftConfirmed) => Ok(Self::Cooldown),
            (Self::Cooldown, ShiftEvent::CooldownElapsed) => Ok(Self::Armed),
            (state, event) => Err(ShiftTransitionError {
                machine: "innovation_shift",
                state,
                event,
            }),
        }
    }
}

/// The detector's fixed bounds: model-revision fields, `Fixed` in this release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShiftConfig {
    pub window: usize,
    pub ratio_lo: f64,
    pub ratio_hi: f64,
    pub confirm: u32,
    pub cooldown: u64,
    pub warmup: u64,
    pub every: u64,
    pub retune_budget: usize,
    pub retune_span: u64,
    /// Bounds on the cumulative scale applied to the initial `R` (its eigenvalue bounds,
    /// since every retune scales `R` uniformly).
    pub scale_min: f64,
    pub scale_max: f64,
}

impl Default for ShiftConfig {
    fn default() -> Self {
        Self {
            window: 32,
            ratio_lo: 0.5,
            ratio_hi: 2.0,
            confirm: 3,
            cooldown: 64,
            warmup: 64,
            every: 32,
            retune_budget: 3,
            retune_span: 1024,
            scale_min: 1.0 / 16.0,
            scale_max: 16.0,
        }
    }
}

impl ShiftConfig {
    fn valid(&self) -> bool {
        self.window > 0
            && self.ratio_lo.is_finite()
            && self.ratio_hi.is_finite()
            && self.ratio_lo > 0.0
            && self.ratio_lo < self.ratio_hi
            && self.confirm > 0
            && self.every > 0
            && self.warmup >= self.window as u64
            && self.scale_min > 0.0
            && self.scale_min <= 1.0
            && self.scale_max >= 1.0
    }
}

/// A confirmed shift and its retune.
#[derive(Clone, Debug, PartialEq)]
pub struct ShiftReport {
    /// Observation count at the trigger.
    pub cursor: u64,
    pub ratio: f64,
    /// Cumulative scale of `R` after the retune.
    pub scale: f64,
    pub old_digest: String,
    pub new_digest: String,
}

/// What one [`StateSpaceFilter::detect_shift`] call decided.
#[derive(Clone, Debug, PartialEq)]
pub enum ShiftOutcome {
    NotDue,
    Cooldown,
    Stable,
    Pending(u32),
    Shifted(ShiftReport),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ShiftDetector {
    cfg: ShiftConfig,
    phase: ShiftPhase,
    baseline: Vec<f64>,
    recent: Vec<f64>,
    count: u64,
    consecutive: u32,
    cooldown_left: u64,
    retunes: Vec<u64>,
    last_checked: u64,
    scale: f64,
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

impl ShiftDetector {
    fn record(&mut self, stat: f64) {
        self.count += 1;
        self.recent.push(stat);
        if self.recent.len() > self.cfg.window {
            self.recent.remove(0);
        }
        match self.phase {
            ShiftPhase::Seeding => {
                self.baseline.push(stat);
                if self.baseline.len() >= self.cfg.window {
                    self.phase = self
                        .phase
                        .transition(ShiftEvent::BaselineFilled)
                        .unwrap_or(self.phase);
                }
            }
            ShiftPhase::Cooldown => {
                self.cooldown_left = self.cooldown_left.saturating_sub(1);
                if self.cooldown_left == 0 {
                    if let Ok(next) = self.phase.transition(ShiftEvent::CooldownElapsed) {
                        self.phase = next;
                        self.baseline = self.recent.clone();
                        self.consecutive = 0;
                    }
                }
            }
            ShiftPhase::Armed => {}
        }
    }
}

// ---------------------------------------------------------------- profiles

/// A volatility band class against the warmup median.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BandClass {
    Low,
    Normal,
    High,
}

/// The mean-reversion outcome of a `phi` estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reversion {
    Theta(f64),
    NonReverting,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Profile {
    Generic,
    Volatility {
        warmup: Vec<f64>,
        median: Option<f64>,
    },
    MeanReversion {
        dt: f64,
        centering_revision: u64,
    },
    Hedge {
        unit: String,
    },
    Factor {
        order: Vec<String>,
    },
}

/// One predict-update step with every intermediate the oracles compare.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterStep {
    pub prior_mean: Vec<f64>,
    pub prior_covariance: Vec<f64>,
    pub innovation: Vec<f64>,
    pub innovation_covariance: Vec<f64>,
    pub gain: Vec<f64>,
    pub mean: Vec<f64>,
    pub covariance: Vec<f64>,
    /// Robust down-weight (`1.0` for the linear update).
    pub weight: f64,
    /// Standardized innovation distance `sqrt(vᵀ S⁻¹ v)`.
    pub distance: f64,
    /// Whether the step linearized a nonlinear profile (no optimality claim then).
    pub approximation: bool,
}

/// A registered nonlinear model: closed, so no caller code runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NonlinearModel {
    /// `f(x) = x`, `h(x) = x`.
    Identity,
    /// `f(x) = x`, `h(x) = x²` on `x > 0`.
    SquaredObservation,
}

impl NonlinearModel {
    /// Resolve a registered model id.
    pub fn from_id(id: &str) -> Res<Self> {
        match id {
            "identity" => Ok(Self::Identity),
            "squared_observation" => Ok(Self::SquaredObservation),
            _ => Err(FilterRefusal::ModelUnregistered),
        }
    }

    fn f(self, x: f64) -> f64 {
        x
    }

    fn df(self, _x: f64) -> f64 {
        1.0
    }

    fn h(self, x: f64) -> f64 {
        match self {
            Self::Identity => x,
            Self::SquaredObservation => x * x,
        }
    }

    fn dh(self, x: f64) -> f64 {
        match self {
            Self::Identity => 1.0,
            Self::SquaredObservation => 2.0 * x,
        }
    }

    fn domain_ok(self, x: f64) -> bool {
        match self {
            Self::Identity => true,
            Self::SquaredObservation => x > 0.0,
        }
    }
}

/// The matrix state-space filter: `x ← F x`, `P ← F P Fᵀ + Q`, then the Joseph-form update.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateSpaceFilter {
    n: usize,
    m: usize,
    f: Vec<f64>,
    q: Vec<f64>,
    r: Vec<f64>,
    r0: Vec<f64>,
    x: Vec<f64>,
    p: Vec<f64>,
    symmetry_tol: f64,
    decomposition_tol: f64,
    profile: Profile,
    detector: Option<ShiftDetector>,
}

impl StateSpaceFilter {
    /// A generic filter; refuses inconsistent shapes, non-finite values and asymmetric or
    /// negative covariances.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        n: usize,
        m: usize,
        f: Vec<f64>,
        q: Vec<f64>,
        r: Vec<f64>,
        x0: Vec<f64>,
        p0: Vec<f64>,
        symmetry_tol: f64,
    ) -> Res<Self> {
        if n == 0 || m == 0 {
            return Err(FilterRefusal::DimensionMismatch);
        }
        let shapes = [
            (f.len(), n * n),
            (q.len(), n * n),
            (r.len(), m * m),
            (x0.len(), n),
            (p0.len(), n * n),
        ];
        if shapes.iter().any(|(got, want)| got != want) {
            return Err(FilterRefusal::DimensionMismatch);
        }
        if ![&f, &q, &r, &x0, &p0].iter().all(|v| finite(v)) || !symmetry_tol.is_finite() {
            return Err(FilterRefusal::InputNonfinite);
        }
        let cov_ok = |a: &[f64], k: usize| {
            (0..k).all(|i| {
                a[i * k + i] >= 0.0
                    && (0..i).all(|j| (a[i * k + j] - a[j * k + i]).abs() <= symmetry_tol)
            })
        };
        if symmetry_tol < 0.0 || !cov_ok(&q, n) || !cov_ok(&r, m) || !cov_ok(&p0, n) {
            return Err(FilterRefusal::ParamInvalid);
        }
        Ok(Self {
            n,
            m,
            f,
            q,
            r0: r.clone(),
            r,
            x: x0,
            p: p0,
            symmetry_tol,
            decomposition_tol: DECOMPOSITION_TOL,
            profile: Profile::Generic,
            detector: None,
        })
    }

    /// The centering revision a mean-reversion model pins.
    pub fn centering_revision(&self) -> Option<u64> {
        match &self.profile {
            Profile::MeanReversion {
                centering_revision, ..
            } => Some(*centering_revision),
            _ => None,
        }
    }

    /// The filtered state mean.
    pub fn mean(&self) -> &[f64] {
        &self.x
    }

    /// The filtered covariance, row-major.
    pub fn covariance(&self) -> &[f64] {
        &self.p
    }

    /// The current measurement noise `R` (after any retune).
    pub fn measurement_noise(&self) -> &[f64] {
        &self.r
    }

    /// Arm the innovation-shift detector with `cfg`.
    pub fn with_shift_detector(mut self, cfg: ShiftConfig) -> Res<Self> {
        if !cfg.valid() {
            return Err(FilterRefusal::WindowInvalid);
        }
        self.detector = Some(ShiftDetector {
            cfg,
            phase: ShiftPhase::Seeding,
            baseline: Vec::new(),
            recent: Vec::new(),
            count: 0,
            consecutive: 0,
            cooldown_left: 0,
            retunes: Vec::new(),
            last_checked: 0,
            scale: 1.0,
        });
        Ok(self)
    }

    /// The detector phase, when armed.
    pub fn shift_phase(&self) -> Option<ShiftPhase> {
        self.detector.as_ref().map(|d| d.phase)
    }

    /// Volatility profile: state is log variance (random walk, noise `q`); the observation
    /// is `ln(r²) + 1.2704` with measurement variance `r`. Seeded at `log_var0`, `p0`.
    pub fn volatility(q: f64, r: f64, log_var0: f64, p0: f64) -> Res<Self> {
        let mut filter = Self::new(
            1,
            1,
            vec![1.0],
            vec![q],
            vec![r],
            vec![log_var0],
            vec![p0],
            SYMMETRY_TOL,
        )?;
        filter.profile = Profile::Volatility {
            warmup: Vec::new(),
            median: None,
        };
        Ok(filter)
    }

    /// Mean-reversion profile: state is `phi` in `next = phi × previous + noise`
    /// (random-walk coefficient). `dt` and the centering revision are required.
    pub fn mean_reversion(
        dt: f64,
        centering_revision: u64,
        q: f64,
        r: f64,
        phi0: f64,
        p0: f64,
    ) -> Res<Self> {
        if !dt.is_finite() || dt <= 0.0 {
            return Err(FilterRefusal::ParamInvalid);
        }
        let mut filter = Self::new(
            1,
            1,
            vec![1.0],
            vec![q],
            vec![r],
            vec![phi0],
            vec![p0],
            SYMMETRY_TOL,
        )?;
        filter.profile = Profile::MeanReversion {
            dt,
            centering_revision,
        };
        Ok(filter)
    }

    /// Hedge-ratio profile: state `[intercept, hedge]`, `H = [1, x]`, one aligned pair per
    /// step in the model's `unit`.
    pub fn hedge(unit: &str, q: f64, r: f64, p0: f64) -> Res<Self> {
        let mut filter = Self::new(
            2,
            1,
            vec![1.0, 0.0, 0.0, 1.0],
            vec![q, 0.0, 0.0, q],
            vec![r],
            vec![0.0, 0.0],
            vec![p0, 0.0, 0.0, p0],
            SYMMETRY_TOL,
        )?;
        filter.profile = Profile::Hedge {
            unit: unit.to_string(),
        };
        Ok(filter)
    }

    /// Factor profile: one coefficient per ordered factor, the order pinned here.
    pub fn factor(order: Vec<String>, q: f64, r: f64, p0: f64) -> Res<Self> {
        let k = order.len();
        let mut f = vec![0.0; k * k];
        let mut qm = vec![0.0; k * k];
        let mut pm = vec![0.0; k * k];
        for i in 0..k {
            f[i * k + i] = 1.0;
            qm[i * k + i] = q;
            pm[i * k + i] = p0;
        }
        let mut filter = Self::new(k, 1, f, qm, vec![r], vec![0.0; k], pm, SYMMETRY_TOL)?;
        filter.profile = Profile::Factor { order };
        Ok(filter)
    }

    // ------------------------------------------------------------ the shared recurrence

    /// Causal predict-update on observation `z` with measurement matrix `h` (`m x n`).
    /// `cross_covariance` declares the process/measurement noise cross-covariance; any
    /// nonzero entry refuses before any update (the gain is only minimum-variance for
    /// uncorrelated noises).
    pub fn update(
        &mut self,
        h: &[f64],
        z: &[f64],
        cross_covariance: Option<&[f64]>,
    ) -> Res<FilterStep> {
        if let Some(cross) = cross_covariance {
            if cross.iter().any(|c| *c != 0.0) {
                return Err(FilterRefusal::NoiseCorrelated);
            }
        }
        if h.len() != self.m * self.n || z.len() != self.m {
            return Err(FilterRefusal::DimensionMismatch);
        }
        if !finite(h) || !finite(z) {
            return Err(FilterRefusal::InputNonfinite);
        }
        let x_minus = matmul(&self.f, self.n, self.n, &self.x, 1);
        let hx = matmul(h, self.m, self.n, &x_minus, 1);
        let v: Vec<f64> = z.iter().zip(&hx).map(|(a, b)| a - b).collect();
        let f = self.f.clone();
        self.step(&f, x_minus, h, v, None, false)
    }

    /// Robust extended update for a registered nonlinear model (`n = m = 1`): linearize once
    /// at the prior state, down-weight the gain by `w = min(1, c / max(d, 1e-12))`.
    pub fn update_robust(&mut self, model: NonlinearModel, z: f64) -> Res<FilterStep> {
        if self.n != 1 || self.m != 1 {
            return Err(FilterRefusal::DimensionMismatch);
        }
        if !z.is_finite() {
            return Err(FilterRefusal::InputNonfinite);
        }
        let x = self.x[0];
        if !model.domain_ok(x) {
            return Err(FilterRefusal::DomainViolation);
        }
        let f_jac = model.df(x);
        let x_minus = model.f(x);
        if !model.domain_ok(x_minus) {
            return Err(FilterRefusal::DomainViolation);
        }
        let h_jac = model.dh(x_minus);
        if !f_jac.is_finite() || !h_jac.is_finite() || !x_minus.is_finite() {
            return Err(FilterRefusal::JacobianInvalid);
        }
        let v = z - model.h(x_minus);
        if !v.is_finite() {
            return Err(FilterRefusal::JacobianInvalid);
        }
        self.step(
            &[f_jac],
            vec![x_minus],
            &[h_jac],
            vec![v],
            Some(ROBUST_CAP),
            true,
        )
    }

    fn step(
        &mut self,
        f: &[f64],
        x_minus: Vec<f64>,
        h: &[f64],
        v: Vec<f64>,
        cap: Option<f64>,
        approximation: bool,
    ) -> Res<FilterStep> {
        let (n, m) = (self.n, self.m);
        if (0..n).any(|i| {
            (0..i).any(|j| (self.p[i * n + j] - self.p[j * n + i]).abs() > self.symmetry_tol)
        }) {
            return Err(FilterRefusal::ParamInvalid);
        }
        let ft = transpose(f, n, n);
        let mut p_minus = matmul(&matmul(f, n, n, &self.p, n), n, n, &ft, n);
        for (pm, q) in p_minus.iter_mut().zip(&self.q) {
            *pm += q;
        }
        symmetrize(&mut p_minus, n);
        let ht = transpose(h, m, n);
        let hp = matmul(h, m, n, &p_minus, n);
        let mut s = matmul(&hp, m, n, &ht, m);
        for (sv, r) in s.iter_mut().zip(&self.r) {
            *sv += r;
        }
        symmetrize(&mut s, m);
        if !finite(&s) || !finite(&p_minus) {
            return Err(FilterRefusal::InputNonfinite);
        }
        let l = cholesky(&s, m, self.decomposition_tol).ok_or(FilterRefusal::Singular)?;
        // Whitened innovation: w = L⁻¹ v, so d² = wᵀw = vᵀ S⁻¹ v.
        let w = forward(&l, m, &v, 1);
        let distance = w.iter().map(|c| c * c).sum::<f64>().sqrt();
        // K = P⁻ Hᵀ S⁻¹ = (S⁻¹ H P⁻)ᵀ; S is solved for, never inverted.
        let y = backward(&l, m, &forward(&l, m, &hp, n), n);
        let mut gain = transpose(&y, m, n);
        let weight = match cap {
            Some(c) => (c / distance.max(1e-12)).min(1.0),
            None => 1.0,
        };
        if weight != 1.0 {
            for g in &mut gain {
                *g *= weight;
            }
        }
        let correction = matmul(&gain, n, m, &v, 1);
        let x_new: Vec<f64> = x_minus
            .iter()
            .zip(&correction)
            .map(|(a, b)| a + b)
            .collect();
        // Joseph form: P = (I − K H) P⁻ (I − K H)ᵀ + K R Kᵀ.
        let kh = matmul(&gain, n, m, h, n);
        let mut ikh = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..n {
                ikh[i * n + j] = f64::from(u8::from(i == j)) - kh[i * n + j];
            }
        }
        let left = matmul(
            &matmul(&ikh, n, n, &p_minus, n),
            n,
            n,
            &transpose(&ikh, n, n),
            n,
        );
        let krkt = matmul(
            &matmul(&gain, n, m, &self.r, m),
            n,
            m,
            &transpose(&gain, n, m),
            n,
        );
        let mut p_new: Vec<f64> = left.iter().zip(&krkt).map(|(a, b)| a + b).collect();
        symmetrize(&mut p_new, n);
        if !finite(&x_new) || !finite(&p_new) {
            return Err(FilterRefusal::InputNonfinite);
        }
        // Commit: nothing above touched `self`.
        let stat = w
            .iter()
            .map(|c| c.clamp(-WHITENED_CLIP, WHITENED_CLIP).powi(2))
            .sum::<f64>()
            / m as f64;
        let prior_mean = x_minus;
        self.x = x_new.clone();
        self.p = p_new.clone();
        if let Some(detector) = self.detector.as_mut() {
            detector.record(stat);
        }
        Ok(FilterStep {
            prior_mean,
            prior_covariance: p_minus,
            innovation: v,
            innovation_covariance: s,
            gain,
            mean: x_new,
            covariance: p_new,
            weight,
            distance,
            approximation,
        })
    }

    // ------------------------------------------------------------ profile observations

    /// Volatility: observe one return. A return whose square is under the floor needs an
    /// admitted `floor` (`STATE_TRANSFORM_INVALID` otherwise). Returns the step, the latent
    /// variance and the band class (`None` until the 60-observation warmup completes).
    pub fn observe_return(
        &mut self,
        ret: f64,
        floor: Option<f64>,
    ) -> Res<(FilterStep, f64, Option<BandClass>)> {
        if !matches!(self.profile, Profile::Volatility { .. }) {
            return Err(FilterRefusal::ModelUnregistered);
        }
        if !ret.is_finite() {
            return Err(FilterRefusal::InputNonfinite);
        }
        let square = ret * ret;
        let used = match floor {
            Some(f) if f.is_finite() && f > 0.0 => square.max(f),
            Some(_) => return Err(FilterRefusal::TransformInvalid),
            None if square >= VOLATILITY_FLOOR => square,
            None => return Err(FilterRefusal::TransformInvalid),
        };
        let y = ln(used) + LOG_CHI2_BIAS;
        let step = self.update(&[1.0], &[y], None)?;
        let variance = exp(step.mean[0]);
        let Profile::Volatility { warmup, median } = &mut self.profile else {
            unreachable!("checked above");
        };
        let class = if let Some(med) = *median {
            Some(if variance < VOLATILITY_LOW_RATIO * med {
                BandClass::Low
            } else if variance > VOLATILITY_HIGH_RATIO * med {
                BandClass::High
            } else {
                BandClass::Normal
            })
        } else {
            warmup.push(variance);
            if warmup.len() >= VOLATILITY_WARMUP {
                let mut sorted = warmup.clone();
                sorted.sort_by(f64::total_cmp);
                let mid = sorted.len() / 2;
                *median = Some(if sorted.len() % 2 == 0 {
                    0.5 * (sorted[mid - 1] + sorted[mid])
                } else {
                    sorted[mid]
                });
            }
            None
        };
        Ok((step, variance, class))
    }

    /// Mean reversion: observe `(previous, next)` of the spread (`H = previous`). The
    /// outcome is `theta = -ln(phi)/dt` only for `0 < phi < 1`.
    pub fn observe_spread(&mut self, previous: f64, next: f64) -> Res<(FilterStep, Reversion)> {
        let Profile::MeanReversion { dt, .. } = self.profile else {
            return Err(FilterRefusal::ModelUnregistered);
        };
        let step = self.update(&[previous], &[next], None)?;
        let phi = step.mean[0];
        let outcome = if phi > 0.0 && phi < 1.0 {
            Reversion::Theta(-ln(phi) / dt)
        } else {
            Reversion::NonReverting
        };
        Ok((step, outcome))
    }

    /// Hedge: one aligned pair (`y` regressed on `x`) in `unit`.
    pub fn observe_pair(&mut self, y: &[f64], x: &[f64], unit: &str) -> Res<FilterStep> {
        let Profile::Hedge { unit: model_unit } = &self.profile else {
            return Err(FilterRefusal::ModelUnregistered);
        };
        if y.len() != 1 || x.len() != 1 {
            return Err(FilterRefusal::DimensionMismatch);
        }
        if unit != model_unit {
            return Err(FilterRefusal::UnitMismatch);
        }
        self.update(&[1.0, x[0]], &[y[0]], None)
    }

    /// Factor: observe `y` at `at_ms` against the factor returns in `order`, each known at
    /// `known_at_ms`. A factor known after `at_ms` is a future leak.
    pub fn observe_factors(
        &mut self,
        y: f64,
        at_ms: i64,
        order: &[String],
        factors: &[f64],
        known_at_ms: &[i64],
    ) -> Res<FilterStep> {
        let Profile::Factor { order: pinned } = &self.profile else {
            return Err(FilterRefusal::ModelUnregistered);
        };
        if factors.len() != self.n
            || known_at_ms.len() != factors.len()
            || order.len() != factors.len()
        {
            return Err(FilterRefusal::DimensionMismatch);
        }
        if order != pinned.as_slice() {
            return Err(FilterRefusal::FactorOrder);
        }
        if known_at_ms.iter().any(|t| *t > at_ms) {
            return Err(FilterRefusal::FutureLeak);
        }
        self.update(factors, &[y], None)
    }

    // ------------------------------------------------------------ innovation-shift detection

    /// Compare the recent whitened-innovation variance with the baseline and, after the
    /// confirmed number of consecutive out-of-band windows, retune `R` within the bounds.
    pub fn detect_shift(&mut self) -> Res<ShiftOutcome> {
        let Some(det) = self.detector.as_mut() else {
            return Err(FilterRefusal::WindowInvalid);
        };
        if det.phase == ShiftPhase::Seeding || det.count < det.cfg.warmup {
            return Err(FilterRefusal::Warmup);
        }
        if det.phase == ShiftPhase::Cooldown {
            return Ok(ShiftOutcome::Cooldown);
        }
        if det.count % det.cfg.every != 0 || det.count == det.last_checked {
            return Ok(ShiftOutcome::NotDue);
        }
        if det.recent.len() != det.cfg.window || det.baseline.len() != det.cfg.window {
            return Err(FilterRefusal::WindowInvalid);
        }
        let ratio = mean(&det.recent) / mean(&det.baseline).max(1e-12);
        det.last_checked = det.count;
        if ratio >= det.cfg.ratio_lo && ratio <= det.cfg.ratio_hi {
            det.consecutive = 0;
            return Ok(ShiftOutcome::Stable);
        }
        det.consecutive += 1;
        if det.consecutive < det.cfg.confirm {
            return Ok(ShiftOutcome::Pending(det.consecutive));
        }
        let recent_retunes = det
            .retunes
            .iter()
            .filter(|at| det.count - **at < det.cfg.retune_span)
            .count();
        if recent_retunes >= det.cfg.retune_budget {
            return Err(FilterRefusal::RetuneBudget);
        }
        let step_scale = ratio.clamp(det.cfg.ratio_lo, det.cfg.ratio_hi);
        let new_scale = (det.scale * step_scale).clamp(det.cfg.scale_min, det.cfg.scale_max);
        let old_digest = fingerprint(&self.r);
        let factor = new_scale / det.scale;
        for r in &mut self.r {
            *r *= factor;
        }
        let new_digest = fingerprint(&self.r);
        det.scale = new_scale;
        det.retunes.push(det.count);
        det.consecutive = 0;
        det.cooldown_left = det.cfg.cooldown;
        if let Ok(next) = det.phase.transition(ShiftEvent::ShiftConfirmed) {
            det.phase = next;
        }
        Ok(ShiftOutcome::Shifted(ShiftReport {
            cursor: det.count,
            ratio,
            scale: new_scale,
            old_digest,
            new_digest,
        }))
    }

    /// The fingerprint of the initial measurement noise (the model revision's parameter).
    pub fn initial_noise_digest(&self) -> String {
        fingerprint(&self.r0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_phase_machine_has_every_legal_and_illegal_transition() {
        use ShiftEvent as E;
        use ShiftPhase as P;
        let legal = [
            (P::Seeding, E::BaselineFilled, P::Armed),
            (P::Armed, E::ShiftConfirmed, P::Cooldown),
            (P::Cooldown, E::CooldownElapsed, P::Armed),
        ];
        for (from, event, to) in legal {
            assert_eq!(from.transition(event), Ok(to));
        }
        for from in [P::Seeding, P::Armed, P::Cooldown] {
            for event in [E::BaselineFilled, E::ShiftConfirmed, E::CooldownElapsed] {
                if legal.iter().any(|(f, e, _)| *f == from && *e == event) {
                    continue;
                }
                let err = from.transition(event).unwrap_err();
                assert_eq!(
                    (err.machine, err.state, err.event),
                    ("innovation_shift", from, event)
                );
            }
        }
    }

    #[test]
    fn retune_budget_and_window_refusals_leave_state_unchanged() {
        let cfg = ShiftConfig {
            window: 4,
            warmup: 4,
            every: 4,
            confirm: 1,
            ..ShiftConfig::default()
        };
        let mut f = StateSpaceFilter::new(
            1,
            1,
            vec![1.0],
            vec![0.0],
            vec![1.0],
            vec![0.0],
            vec![1.0],
            SYMMETRY_TOL,
        )
        .unwrap()
        .with_shift_detector(cfg)
        .unwrap();
        for i in 0..4 {
            f.update(&[1.0], &[if i % 2 == 0 { 1.0 } else { -1.0 }], None)
                .unwrap();
        }
        assert_eq!(f.detect_shift(), Ok(ShiftOutcome::Stable));
        for _ in 0..4 {
            f.update(&[1.0], &[9.0], None).unwrap();
        }
        let det = f.detector.as_mut().unwrap();
        det.retunes = vec![det.count - 1, det.count - 2, det.count - 3];
        let before = f.clone();
        assert_eq!(f.detect_shift(), Err(FilterRefusal::RetuneBudget));
        assert_eq!(f.measurement_noise(), before.measurement_noise());
        assert_eq!(f.shift_phase(), before.shift_phase());
        let bad = ShiftConfig {
            window: 0,
            ..ShiftConfig::default()
        };
        let built = StateSpaceFilter::volatility(0.1, 1.0, 0.0, 1.0)
            .unwrap()
            .with_shift_detector(bad);
        assert_eq!(built.unwrap_err(), FilterRefusal::WindowInvalid);
    }
}
