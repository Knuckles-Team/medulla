//! Statistical-arbitrage kernels (MEDULLA-FINANCE-PRIMITIVES-R002.1.11..R002.1.13), re-homed from the
//! finance module: the log-variance Kalman volatility tracker, the Augmented
//! Dickey-Fuller test and Ornstein-Uhlenbeck calibration with its MFPT-optimal
//! entry band. One implementation; the finance Methods and `NumericEvaluate`
//! both call it. Transcendentals go through the pinned `libm` (`detkernel::math`).
//!
//! Primary sources: Kalman (1960); Engle-Granger / MacKinnon (1991) ADF critical
//! values; Ornstein-Uhlenbeck mean reversion with first-passage-time thresholds.

use medulla_types::numeric_kernel::{AdfResult, KalmanState, OuParams, OuThresholds};
use nalgebra::{DMatrix, DVector};

use crate::detkernel::math;
use crate::series::kalman::Kalman;
use crate::series::KalmanNoise;

/// `ln`, `exp` and `x²` through the pinned software math library.
trait Det {
    fn dln(self) -> f64;
    fn dexp(self) -> f64;
}

impl Det for f64 {
    fn dln(self) -> f64 {
        math::ln(self)
    }
    fn dexp(self) -> f64 {
        math::exp(self)
    }
}

/// Ordinary least squares returning coefficients, their standard errors, and the
/// residual variance. `x` rows are observations, columns are regressors (caller
/// supplies the intercept column explicitly if wanted).
pub(crate) fn ols_with_se(x: &[Vec<f64>], y: &[f64]) -> Option<(Vec<f64>, Vec<f64>, f64)> {
    let n = x.len();
    if n == 0 {
        return None;
    }
    let k = x[0].len();
    if n <= k {
        return None;
    }
    let xm = DMatrix::from_fn(n, k, |i, j| x[i][j]);
    let yv = DVector::from_fn(n, |i, _| y[i]);
    let xtx = xm.transpose() * &xm;
    let xtx_inv = xtx.try_inverse()?;
    let beta = &xtx_inv * xm.transpose() * &yv;
    let resid = &yv - &xm * &beta;
    let rss: f64 = resid.iter().map(|e| e * e).sum();
    let dof = (n - k) as f64;
    let sigma2 = rss / dof;
    let ses: Vec<f64> = (0..k).map(|j| (sigma2 * xtx_inv[(j, j)]).sqrt()).collect();
    Some((beta.iter().copied().collect(), ses, sigma2))
}

/// Scalar Kalman filter with constant matrices: x_t = F x_{t-1} + w (Q);
/// z_t = H x_t + v (R). Returns the filtered state + variance at each step.
pub fn kalman_filter_1d(
    observations: &[f64],
    f: f64,
    q: f64,
    h: f64,
    r: f64,
    x0: f64,
    p0: f64,
) -> KalmanState {
    let mut filter = Kalman::new(f, KalmanNoise { q, r }, x0, p0);
    let (states, variances) = observations.iter().map(|&z| filter.observe(z, h)).unzip();
    KalmanState { states, variances }
}

/// Dynamic beta via Kalman filter: hidden state β follows a random walk
/// (process noise q), measurement is r_asset = β · r_market + v (noise r), so the
/// measurement matrix is time-varying H_t = r_market,t. Returns β + variance series.
pub fn kalman_beta(
    market_returns: &[f64],
    asset_returns: &[f64],
    q: f64,
    r: f64,
    beta0: f64,
    p0: f64,
) -> KalmanState {
    // Random-walk β (F = 1) observed through the time-varying H = the market return.
    let mut filter = Kalman::new(1.0, KalmanNoise { q, r }, beta0, p0);
    let (states, variances) = market_returns
        .iter()
        .zip(asset_returns)
        .map(|(&h, &z)| filter.observe(z, h))
        .unzip();
    KalmanState { states, variances }
}

/// Kalman volatility tracker. Hidden state is log-variance (random walk, noise q);
/// measurement is log(r_t²) = log σ²_t + η. Returns the ANNUALISED volatility
/// series (√(σ²·252)). `log_var0=None` seeds from the first ≤60 observations.
pub fn kalman_volatility(
    returns: &[f64],
    q: f64,
    r: f64,
    log_var0: Option<f64>,
    p0: f64,
    annualization: f64,
) -> Vec<f64> {
    let n = returns.len();
    if n == 0 {
        return vec![];
    }
    let log_sq: Vec<f64> = returns.iter().map(|x| (x * x).max(1e-12).dln()).collect();
    let seed = log_var0.unwrap_or_else(|| {
        let m = n.min(60);
        log_sq[..m].iter().sum::<f64>() / m as f64
    });
    let (mut log_var, mut p) = (seed, p0);
    let mut out = vec![0.0; n];
    for t in 0..n {
        p += q;
        let y = log_sq[t] - log_var;
        let s = p + r;
        let k = if s.abs() > 1e-18 { p / s } else { 0.0 };
        log_var += k * y;
        p *= 1.0 - k;
        out[t] = (log_var.dexp() * annualization).sqrt();
    }
    out
}

/// Finite-sample MacKinnon critical value for the ADF "constant, no trend" case:
/// CV(T) = β∞ + β1/T + β2/T² (MacKinnon 1991 response-surface coefficients).
fn mackinnon_crit(level: u8, t: f64) -> f64 {
    // (β∞, β1, β2) per significance level for the τ_c (constant) case.
    let (b0, b1, b2) = match level {
        1 => (-3.43035, -6.5393, -16.786),
        5 => (-2.86154, -2.8903, -4.234),
        _ => (-2.56677, -1.5384, -2.809), // 10%
    };
    b0 + b1 / t + b2 / (t * t)
}

/// Monotone approximate p-value from the ADF statistic and the three interpolated
/// critical values. Piecewise-linear in the statistic; clearly an approximation
/// (the exact value needs MacKinnon's full surface), but correct in ordering and
/// bracket — more useful than a single fixed cutoff.
fn adf_pvalue(stat: f64, c1: f64, c5: f64, c10: f64) -> f64 {
    // anchors: (stat, p). More-negative stat ⇒ smaller p.
    if stat <= c1 {
        return (0.01 * (stat / c1).clamp(0.0, 1.0)).clamp(1e-4, 0.01);
    }
    let interp = |s: f64, lo_s: f64, hi_s: f64, lo_p: f64, hi_p: f64| {
        lo_p + (s - lo_s) / (hi_s - lo_s) * (hi_p - lo_p)
    };
    if stat <= c5 {
        return interp(stat, c1, c5, 0.01, 0.05);
    }
    if stat <= c10 {
        return interp(stat, c5, c10, 0.05, 0.10);
    }
    // above 10% critical: p rises toward ~1 as stat → 0+
    interp(stat, c10, 0.0, 0.10, 0.90).clamp(0.10, 0.999)
}

/// Augmented Dickey-Fuller test (constant, no trend). Regresses
/// Δy_t = α + γ·y_{t-1} + Σ δ_i Δy_{t-i} + ε; the ADF statistic is the t-stat on γ.
pub fn adf_test(series: &[f64], max_lag: usize) -> AdfResult {
    let n = series.len();
    // need enough points after differencing + lags
    if n < max_lag + 4 {
        return AdfResult {
            statistic: 0.0,
            used_lag: max_lag,
            n_obs: 0,
            crit_1pct: -3.43,
            crit_5pct: -2.86,
            crit_10pct: -2.57,
            p_value_approx: 1.0,
            stationary_1pct: false,
            stationary_5pct: false,
            stationary_10pct: false,
        };
    }
    let dy: Vec<f64> = (1..n).map(|i| series[i] - series[i - 1]).collect();
    // build design: rows from t=max_lag .. dy.len()-1
    let start = max_lag;
    let mut xrows: Vec<Vec<f64>> = vec![];
    let mut yvec: Vec<f64> = vec![];
    for t in start..dy.len() {
        let mut row = vec![1.0, series[t]]; // intercept, y_{t-1} (series index t aligns with dy[t]=series[t+1]-series[t])
        for i in 1..=max_lag {
            row.push(dy[t - i]);
        }
        xrows.push(row);
        yvec.push(dy[t]);
    }
    let (stat, n_obs) = match ols_with_se(&xrows, &yvec) {
        Some((coefs, ses, _)) => {
            // coefs[1] is γ on y_{t-1}; t-stat = γ / se(γ)
            let gamma = coefs[1];
            let se = ses[1];
            let t = if se.abs() > 1e-18 { gamma / se } else { 0.0 };
            (t, yvec.len())
        }
        None => (0.0, 0),
    };
    let t = n_obs.max(1) as f64;
    let c1 = mackinnon_crit(1, t);
    let c5 = mackinnon_crit(5, t);
    let c10 = mackinnon_crit(10, t);
    AdfResult {
        statistic: stat,
        used_lag: max_lag,
        n_obs,
        crit_1pct: c1,
        crit_5pct: c5,
        crit_10pct: c10,
        p_value_approx: adf_pvalue(stat, c1, c5, c10),
        stationary_1pct: stat < c1,
        stationary_5pct: stat < c5,
        stationary_10pct: stat < c10,
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Ornstein-Uhlenbeck: calibration + MFPT-optimal thresholds
// ════════════════════════════════════════════════════════════════════════

/// Calibrate an OU process dS = θ(μ−S)dt + σ dW from a discretely-sampled spread
/// via the exact AR(1) discretisation S_t = a + b·S_{t-1} + ε (Euler-Maruyama /
/// MLE-equivalent). `dt` is the sampling interval.
/// The fitted AR(1) coefficient `phi` of a spread, unclamped; `None` when the
/// regression is singular. A reverting fit has `0 < phi < 1`.
pub fn ou_phi(spread: &[f64]) -> Option<f64> {
    let n = spread.len();
    if n < 3 {
        return None;
    }
    let x: Vec<Vec<f64>> = (0..n - 1).map(|i| vec![1.0, spread[i]]).collect();
    let y: Vec<f64> = (1..n).map(|i| spread[i]).collect();
    ols_with_se(&x, &y).map(|(coefs, _, _)| coefs[1])
}

pub fn ou_calibrate(spread: &[f64], dt: f64) -> OuParams {
    let n = spread.len();
    if n < 3 {
        return OuParams {
            theta: 0.0,
            mu: spread.iter().sum::<f64>() / n.max(1) as f64,
            sigma: 0.0,
            half_life: f64::INFINITY,
            sigma_eq: 0.0,
        };
    }
    let x: Vec<Vec<f64>> = (0..n - 1).map(|i| vec![1.0, spread[i]]).collect();
    let y: Vec<f64> = (1..n).map(|i| spread[i]).collect();
    let (a, b, resid_var) = match ols_with_se(&x, &y) {
        Some((coefs, _, sigma2)) => (coefs[0], coefs[1].clamp(-0.999_999, 0.999_999), sigma2),
        None => (0.0, 0.0, 0.0),
    };
    let theta = if b > 0.0 { -b.dln() / dt } else { 1.0 / dt };
    let mu = if (1.0 - b).abs() > 1e-9 {
        a / (1.0 - b)
    } else {
        y.iter().sum::<f64>() / y.len() as f64
    };
    // σ from residual variance: Var(ε) = σ²(1−e^{-2θΔt})/(2θ)
    let denom = 1.0 - (-2.0 * theta * dt).dexp();
    let sigma = if denom > 1e-12 {
        (resid_var * 2.0 * theta / denom).sqrt()
    } else {
        resid_var.sqrt()
    };
    let sigma_eq = if theta > 1e-12 {
        sigma / (2.0 * theta).sqrt()
    } else {
        sigma
    };
    OuParams {
        theta,
        mu,
        sigma,
        half_life: if theta > 1e-12 {
            std::f64::consts::LN_2 / theta
        } else {
            f64::INFINITY
        },
        sigma_eq,
    }
}

/// Expected first-passage time (in σ_eq units) of a normalised OU from deviation
/// `b` back to the mean (0), solved from the backward-Kolmogorov MFPT ODE
/// u'' − z·u' = −1/θ on [0, b] with u(0)=0 (exit at mean) and u'(b)=0 (reflecting
/// at entry), via a tridiagonal finite-difference solve.
fn ou_mfpt(theta: f64, b: f64, n: usize) -> f64 {
    if b <= 0.0 || theta <= 0.0 {
        return f64::INFINITY;
    }
    let h = b / n as f64;
    // unknowns u_1..u_n (u_0 = 0). Node i at z_i = i*h.
    // interior i=1..n-1: (u_{i+1}-2u_i+u_{i-1})/h² − z_i(u_{i+1}-u_{i-1})/(2h) = −1/θ
    // boundary i=n: reflecting u'(b)=0 → ghost u_{n+1}=u_{n-1}; gives
    //   (2u_{n-1}-2u_n)/h² = −1/θ
    let m = n; // unknown count (indices 1..=n)
    let mut lower = vec![0.0; m];
    let mut diag = vec![0.0; m];
    let mut upper = vec![0.0; m];
    let mut rhs = vec![0.0; m];
    let inv_h2 = 1.0 / (h * h);
    for i in 1..=n {
        let z = i as f64 * h;
        let row = i - 1;
        if i < n {
            let a_low = inv_h2 + z / (2.0 * h);
            let a_diag = -2.0 * inv_h2;
            let a_up = inv_h2 - z / (2.0 * h);
            lower[row] = a_low;
            diag[row] = a_diag;
            upper[row] = a_up;
            rhs[row] = -1.0 / theta;
            if i == 1 {
                // u_0 = 0 → drop lower contribution
                lower[row] = 0.0;
            }
        } else {
            // reflecting boundary at i=n
            lower[row] = 2.0 * inv_h2;
            diag[row] = -2.0 * inv_h2;
            upper[row] = 0.0;
            rhs[row] = -1.0 / theta;
        }
    }
    // Thomas algorithm
    for i in 1..m {
        let w = lower[i] / diag[i - 1];
        diag[i] -= w * upper[i - 1];
        rhs[i] -= w * rhs[i - 1];
    }
    let mut u = vec![0.0; m];
    u[m - 1] = rhs[m - 1] / diag[m - 1];
    for i in (0..m - 1).rev() {
        u[i] = (rhs[i] - upper[i] * u[i + 1]) / diag[i];
    }
    u[m - 1] // MFPT from entry (z=b) to mean
}

/// MFPT-optimal OU entry/exit band. Grid-searches the entry deviation z (in σ_eq
/// units) that maximises expected profit per unit time
/// J(z) = (z·σ_eq − cost) / MFPT(z), capturing the move from entry back to the mean.
pub fn ou_optimal_thresholds(params: &OuParams, cost: f64) -> OuThresholds {
    let s = params.sigma_eq;
    let theta = params.theta;
    let (best_z, best_j) = if s > 1e-12 && theta > 1e-12 {
        search_best_entry_z(theta, s, cost)
    } else {
        (1.0, f64::NEG_INFINITY)
    };
    OuThresholds {
        entry_long: params.mu - best_z * s,
        entry_short: params.mu + best_z * s,
        exit: params.mu,
        z: best_z,
        expected_return_per_unit_time: if best_j.is_finite() { best_j } else { 0.0 },
    }
}

/// Grid-search the entry deviation `z` (in `σ_eq` units) maximizing expected
/// profit per unit time `J(z) = (z·σ_eq − cost) / MFPT(z)`. Returns `(best_z, best_j)`.
fn search_best_entry_z(theta: f64, s: f64, cost: f64) -> (f64, f64) {
    let mut best_z = 1.0;
    let mut best_j = f64::NEG_INFINITY;
    let steps = 60;
    for i in 1..=steps {
        let z = 0.05 * i as f64; // up to 3.0 σ_eq
        let profit = z * s - cost;
        if profit <= 0.0 {
            continue;
        }
        let t = ou_mfpt(theta, z, 50);
        if !t.is_finite() || t <= 0.0 {
            continue;
        }
        let j = profit / t;
        if j > best_j {
            best_j = j;
            best_z = z;
        }
    }
    (best_z, best_j)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise in [-1, 1).
    fn noise_source(mut seed: u64) -> impl FnMut() -> f64 {
        move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.12
    #[test]
    fn ou_independent_vectors() {
        use crate::evaluation::registry::evaluate;
        use medulla_types::numeric_kernel::{KernelCode, NumericEvaluateOp as Op};
        // s_t = 0.5 s_{t-1} + 1 has phi = 0.5, mean 2 and theta = -ln(0.5)/dt = ln 2.
        let spread = [0.0, 1.0, 1.5, 1.75, 1.875, 1.9375];
        let phi = ou_phi(&spread).unwrap();
        assert!((phi - 0.5).abs() < 1e-9);
        let p = ou_calibrate(&spread, 1.0);
        assert!((p.theta - std::f64::consts::LN_2).abs() < 1e-6);
        assert!((p.mu - 2.0).abs() < 1e-6);
        assert!((p.half_life - 1.0).abs() < 1e-6);
        // dt scales theta as -ln(phi)/dt.
        let p = ou_calibrate(&spread, 0.5);
        assert!((p.theta - 2.0 * std::f64::consts::LN_2).abs() < 1e-6);
        // A seeded noisy AR(1) path recovers phi within sampling error.
        let mut noise = noise_source(7);
        let mut path = vec![0.0];
        for i in 1..4_000 {
            path.push(0.8 * path[i - 1] + 0.1 * noise());
        }
        assert!((ou_phi(&path).unwrap() - 0.8).abs() < 0.05);
        // Refusals: short input, bad dt, non-reverting fits.
        let code = |op: Op| evaluate(&op).unwrap_err().code;
        assert_eq!(
            code(Op::OuCalibrate {
                spread: vec![1.0, 2.0],
                dt: 1.0
            }),
            KernelCode::InputShort
        );
        assert_eq!(
            code(Op::OuCalibrate {
                spread: spread.to_vec(),
                dt: 0.0
            }),
            KernelCode::ParamInvalid
        );
        // phi = 2 (explosive) and phi = -0.5 (oscillating) are not reverting.
        assert_eq!(
            code(Op::OuCalibrate {
                spread: vec![1.0, 2.0, 4.0, 8.0, 16.0],
                dt: 1.0
            }),
            KernelCode::Singular
        );
        assert_eq!(
            code(Op::OuCalibrate {
                spread: vec![16.0, -8.0, 4.0, -2.0, 1.0],
                dt: 1.0
            }),
            KernelCode::Singular
        );
        assert_eq!(
            code(Op::OuCalibrate {
                spread: vec![3.0; 6],
                dt: 1.0
            }),
            KernelCode::Singular
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.13
    #[test]
    fn adf_independent_vectors() {
        use crate::evaluation::registry::evaluate;
        use medulla_types::numeric_kernel::{KernelCode, NumericEvaluateOp as Op};
        let mut noise = noise_source(12_345);
        let mut stationary = vec![0.0];
        for i in 1..400 {
            stationary.push(0.2 * stationary[i - 1] + noise());
        }
        let mut walk = vec![0.0];
        for _ in 1..400 {
            walk.push(walk.last().unwrap() + noise());
        }
        let ar = adf_test(&stationary, 1);
        assert!(ar.stationary_5pct && ar.statistic < ar.crit_5pct);
        assert!(ar.crit_1pct < ar.crit_5pct && ar.crit_5pct < ar.crit_10pct);
        assert_eq!(ar.n_obs, 398);
        let rw = adf_test(&walk, 1);
        assert!(!rw.stationary_5pct && rw.statistic > rw.crit_5pct);
        // The finite-sample critical values approach MacKinnon's asymptotic ones.
        assert!((ar.crit_5pct + 2.86).abs() < 0.02);
        let code = |op: Op| evaluate(&op).unwrap_err().code;
        assert_eq!(
            code(Op::Adf {
                series: vec![1.0, 2.0, 3.0],
                max_lag: 0
            }),
            KernelCode::InputShort
        );
        // max_lag above n/4 refuses.
        assert_eq!(
            code(Op::Adf {
                series: stationary[..40].to_vec(),
                max_lag: 11
            }),
            KernelCode::ParamInvalid
        );
        assert!(evaluate(&Op::Adf {
            series: stationary[..40].to_vec(),
            max_lag: 10
        })
        .is_ok());
        assert_eq!(
            code(Op::Adf {
                series: vec![5.0; 30],
                max_lag: 1
            }),
            KernelCode::Singular
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.11
    #[test]
    fn kalman_volatility_independent_vectors() {
        // A seeded log-variance that never moves: sqrt(exp(ln 0.01) * 252).
        let v = kalman_volatility(&[0.1, -0.1, 0.1], 0.0, 1.0, Some(0.01_f64.ln()), 1.0, 252.0);
        for x in v {
            assert!((x - 2.52_f64.sqrt()).abs() < 1e-9);
        }
        assert!(kalman_volatility(&[], 0.0, 1.0, None, 1.0, 252.0).is_empty());
    }
}
