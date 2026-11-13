//! The scalar Kalman filter (EH-530, re-homed from the finance state-space module): one
//! recurrence behind the local-level and dynamic-beta series kernels and the finance
//! `kalman_filter_1d` / `kalman_beta` Methods.

use serde::{Deserialize, Serialize};

use super::KalmanNoise;

pub use super::state_space::*;

/// Below this the innovation variance is treated as zero (no update).
const S_FLOOR: f64 = 1e-18;

/// `x_t = F·x_{t−1} + w (Q)`, `z_t = H_t·x_t + v (R)`: the filtered state and variance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kalman {
    f: f64,
    q: f64,
    r: f64,
    /// `None` until seeded: a local-level filter seeds with its first observation.
    x: Option<f64>,
    p: f64,
}

impl Kalman {
    /// A filter with explicit transition `f` and initial state `x0` at variance `p0`.
    pub fn new(f: f64, noise: KalmanNoise, x0: f64, p0: f64) -> Self {
        Self {
            f,
            q: noise.q,
            r: noise.r,
            x: Some(x0),
            p: p0,
        }
    }

    /// A random-walk level seeded with its first observation at variance `r`.
    pub fn level(noise: KalmanNoise) -> Self {
        Self {
            f: 1.0,
            q: noise.q,
            r: noise.r,
            x: None,
            p: noise.r,
        }
    }

    /// A random-walk regression coefficient starting at `β = 0`, variance 1.
    pub fn beta(noise: KalmanNoise) -> Self {
        Self::new(1.0, noise, 0.0, 1.0)
    }

    /// Predict, then update on observation `z` with measurement coefficient `h`; the
    /// filtered `(state, variance)`.
    pub fn observe(&mut self, z: f64, h: f64) -> (f64, f64) {
        let Some(mut x) = self.x else {
            self.x = Some(z);
            return (z, self.p);
        };
        x *= self.f;
        let mut p = self.f * self.p * self.f + self.q;
        let s = h * p * h + self.r;
        let k = if s.abs() > S_FLOOR { p * h / s } else { 0.0 };
        x += k * (z - h * x);
        p *= 1.0 - k * h;
        self.x = Some(x);
        self.p = p;
        (x, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detkernel::math::ln;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} != {b}");
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.11
    #[cfg(feature = "evaluate")]
    #[test]
    fn kalman_independent_vectors() {
        use crate::evaluation::registry::evaluate;
        use medulla_types::numeric_kernel::{
            KernelCode, NumericEvaluateOp as Op, NumericKernelOutput,
        };
        // Level, q = 0, r = 1, p0 = 1, observations [2, 4], worked by hand:
        // step 1: P = 1, S = 2, K = 1/2, x = 1, P = 1/2; step 2: S = 3/2, K = 1/3, x = 2, P = 1/3.
        let mut level = Kalman::new(1.0, KalmanNoise { q: 0.0, r: 1.0 }, 0.0, 1.0);
        let (x, p) = level.observe(2.0, 1.0);
        close(x, 1.0);
        close(p, 0.5);
        let (x, p) = level.observe(4.0, 1.0);
        close(x, 2.0);
        close(p, 1.0 / 3.0);
        // The seeded level takes its first observation as the state at variance r.
        let mut seeded = Kalman::level(KalmanNoise { q: 0.0, r: 1.0 });
        assert_eq!(seeded.observe(5.0, 1.0), (5.0, 1.0));
        // Beta, H = [1, 2], z = [2, 4]: step 2 has S = 3, K = 1/3, x = 5/3, P = 1/6.
        let mut beta = Kalman::beta(KalmanNoise { q: 0.0, r: 1.0 });
        beta.observe(2.0, 1.0);
        let (x, p) = beta.observe(4.0, 2.0);
        close(x, 5.0 / 3.0);
        close(p, 1.0 / 6.0);

        // The registry serves the same vectors and refuses with each stable code.
        let served = evaluate(&Op::KalmanLevel {
            observations: vec![2.0, 4.0],
            f: 1.0,
            q: 0.0,
            h: 1.0,
            r: 1.0,
            x0: 0.0,
            p0: 1.0,
        })
        .unwrap();
        let NumericKernelOutput::Kalman(state) = served else {
            panic!("expected a Kalman state")
        };
        close(state.states[1], 2.0);
        close(state.variances[1], 1.0 / 3.0);
        let level = |observations: Vec<f64>, q: f64, r: f64, p0: f64| {
            evaluate(&Op::KalmanLevel {
                observations,
                f: 1.0,
                q,
                h: 1.0,
                r,
                x0: 0.0,
                p0,
            })
        };
        assert_eq!(
            level(vec![], 0.0, 1.0, 1.0).unwrap_err().code,
            KernelCode::InputShort
        );
        assert_eq!(
            level(vec![1.0], -1.0, 1.0, 1.0).unwrap_err().code,
            KernelCode::ParamInvalid
        );
        assert_eq!(
            level(vec![1.0], 0.0, 0.0, 0.0).unwrap_err().code,
            KernelCode::Singular
        );
        let beta = evaluate(&Op::KalmanBeta {
            market_returns: vec![1.0, 2.0],
            asset_returns: vec![2.0, 4.0],
            q: 0.0,
            r: 0.0,
            beta0: 0.0,
            p0: 0.0,
        });
        assert_eq!(beta.unwrap_err().code, KernelCode::Singular);
    }

    // ---- state-space recurrence (R002.6, R002.7, R002.8) ------------------------------------

    struct Lcg(u64);
    impl Lcg {
        /// Uniform in (-sqrt(3), sqrt(3)): unit variance.
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 3f64.sqrt()
        }
    }

    /// Independent oracle for `F = I`, `Q = q I`, scalar observation: textbook (non-Joseph)
    /// covariance update with an explicit scalar innovation variance.
    fn oracle(
        x: &mut Vec<f64>,
        p: &mut Vec<f64>,
        q: f64,
        r: f64,
        h: &[f64],
        z: f64,
    ) -> [Vec<f64>; 5] {
        let n = x.len();
        let mut pm = p.clone();
        for i in 0..n {
            pm[i * n + i] += q;
        }
        let prior = x.clone();
        let v = z - (0..n).map(|i| h[i] * x[i]).sum::<f64>();
        let ph: Vec<f64> = (0..n)
            .map(|i| (0..n).map(|j| pm[i * n + j] * h[j]).sum())
            .collect();
        let s = (0..n).map(|i| h[i] * ph[i]).sum::<f64>() + r;
        let k: Vec<f64> = ph.iter().map(|c| c / s).collect();
        for i in 0..n {
            x[i] += k[i] * v;
        }
        let hp: Vec<f64> = (0..n)
            .map(|j| (0..n).map(|i| h[i] * pm[i * n + j]).sum())
            .collect();
        for i in 0..n {
            for j in 0..n {
                p[i * n + j] = pm[i * n + j] - k[i] * hp[j];
            }
        }
        [prior, vec![v], k, x.clone(), p.clone()]
    }

    fn near(a: &[f64], b: &[f64]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= 1e-9 * (1.0 + y.abs()), "{a:?} != {b:?}");
        }
    }

    fn check_step(step: &FilterStep, want: &[Vec<f64>; 5]) {
        near(&step.prior_mean, &want[0]);
        near(&step.innovation, &want[1]);
        near(&step.gain, &want[2]);
        near(&step.mean, &want[3]);
        near(&step.covariance, &want[4]);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6.1
    #[test]
    fn causal_linear_recurrence_and_noise_refusal() {
        let mut matrix = StateSpaceFilter::new(
            1,
            1,
            vec![1.0],
            vec![0.0],
            vec![1.0],
            vec![0.0],
            vec![1.0],
            SYMMETRY_TOL,
        )
        .unwrap();
        let mut scalar = Kalman::new(1.0, KalmanNoise { q: 0.0, r: 1.0 }, 0.0, 1.0);
        for (i, z) in [2.0, 4.0, -1.0, 0.5].into_iter().enumerate() {
            let step = matrix.update(&[1.0], &[z], None).unwrap();
            let (x, p) = scalar.observe(z, 1.0);
            close(step.mean[0], x);
            close(step.covariance[0], p);
            if i == 1 {
                // The rational vector of the scalar suite: x = 2, P = 1/3.
                close(step.mean[0], 2.0);
                close(step.covariance[0], 1.0 / 3.0);
            }
            // The checkpoint (serde) continues exactly.
            let restored: StateSpaceFilter =
                serde_json::from_str(&serde_json::to_string(&matrix).unwrap()).unwrap();
            assert_eq!(restored, matrix);
        }
        // Q = 1, R = 4, cross-covariance = 1: refused before any update, state unchanged.
        let mut correlated = StateSpaceFilter::new(
            1,
            1,
            vec![1.0],
            vec![1.0],
            vec![4.0],
            vec![0.0],
            vec![1.0],
            SYMMETRY_TOL,
        )
        .unwrap();
        let before = correlated.clone();
        assert_eq!(
            correlated.update(&[1.0], &[3.0], Some(&[1.0])).unwrap_err(),
            FilterRefusal::NoiseCorrelated
        );
        assert_eq!(correlated, before);
        assert!(correlated.update(&[1.0], &[3.0], Some(&[0.0])).is_ok());
        // A singular innovation covariance refuses and keeps the state.
        let mut singular = StateSpaceFilter::new(
            1,
            1,
            vec![1.0],
            vec![0.0],
            vec![0.0],
            vec![0.0],
            vec![0.0],
            SYMMETRY_TOL,
        )
        .unwrap();
        let before = singular.clone();
        assert_eq!(
            singular.update(&[1.0], &[1.0], None).unwrap_err(),
            FilterRefusal::Singular
        );
        assert_eq!(singular, before);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6.2
    #[test]
    fn volatility_profile_oracle() {
        let mut rng = Lcg(7);
        let returns: Vec<f64> = (0..90).map(|_| 0.01 * rng.next()).collect();
        let run = |n: usize| {
            let mut f = StateSpaceFilter::volatility(0.01, 4.9348, -9.0, 1.0).unwrap();
            let (mut x, mut p) = (vec![-9.0], vec![1.0]);
            let mut classes = Vec::new();
            let mut means = Vec::new();
            for r in &returns[..n] {
                let (step, variance, class) = f.observe_return(*r, None).unwrap();
                let y = ln((r * r).max(VOLATILITY_FLOOR)) + LOG_CHI2_BIAS;
                check_step(&step, &oracle(&mut x, &mut p, 0.01, 4.9348, &[1.0], y));
                close(variance, x[0].exp());
                classes.push(class);
                means.push(step.mean[0].to_bits());
            }
            (classes, means)
        };
        let (classes, whole) = run(90);
        assert!(
            classes[..60].iter().all(Option::is_none),
            "warmup is 60 observations"
        );
        assert!(classes[60..].iter().all(Option::is_some));
        // Future observations never change an earlier prefix.
        assert_eq!(run(40).1, whole[..40]);
        // A zero return needs an admitted floor; invalid floors refuse; state is kept.
        let mut f = StateSpaceFilter::volatility(0.01, 4.9348, -9.0, 1.0).unwrap();
        let before = f.clone();
        assert_eq!(
            f.observe_return(0.0, None).unwrap_err(),
            FilterRefusal::TransformInvalid
        );
        assert_eq!(
            f.observe_return(0.0, Some(0.0)).unwrap_err(),
            FilterRefusal::TransformInvalid
        );
        assert_eq!(
            f.observe_return(f64::NAN, None).unwrap_err(),
            FilterRefusal::InputNonfinite
        );
        assert_eq!(f, before);
        assert!(f.observe_return(0.0, Some(1e-12)).is_ok());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6.3
    #[test]
    fn mean_reversion_profile_oracle() {
        let mut rng = Lcg(11);
        let mut spread = vec![0.0];
        for _ in 0..80 {
            spread.push(0.9 * spread.last().unwrap() + 0.1 * rng.next());
        }
        let run = |n: usize| {
            let mut f = StateSpaceFilter::mean_reversion(1.0, 3, 1e-6, 0.01, 0.5, 1.0).unwrap();
            assert_eq!(f.centering_revision(), Some(3));
            let (mut x, mut p) = (vec![0.5], vec![1.0]);
            let mut bits = Vec::new();
            for w in spread.windows(2).take(n) {
                let (step, outcome) = f.observe_spread(w[0], w[1]).unwrap();
                check_step(&step, &oracle(&mut x, &mut p, 1e-6, 0.01, &[w[0]], w[1]));
                match outcome {
                    Reversion::Theta(t) => {
                        assert!(x[0] > 0.0 && x[0] < 1.0);
                        close(t, -x[0].ln());
                    }
                    Reversion::NonReverting => assert!(!(x[0] > 0.0 && x[0] < 1.0)),
                }
                bits.push(step.mean[0].to_bits());
            }
            bits
        };
        let whole = run(80);
        assert_eq!(run(30), whole[..30]);
        // phi at or above 1 is the typed non-reverting outcome.
        let mut f = StateSpaceFilter::mean_reversion(1.0, 0, 0.0, 1e6, 1.5, 1e-9).unwrap();
        assert_eq!(
            f.observe_spread(1.0, 1.0).unwrap().1,
            Reversion::NonReverting
        );
        for dt in [0.0, -1.0, f64::NAN] {
            assert_eq!(
                StateSpaceFilter::mean_reversion(dt, 0, 0.0, 1.0, 0.5, 1.0).unwrap_err(),
                FilterRefusal::ParamInvalid
            );
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6.4
    #[test]
    fn hedge_profile_oracle() {
        let mut rng = Lcg(13);
        let pairs: Vec<(f64, f64)> = (0..60)
            .map(|_| {
                let x = rng.next();
                (0.3 + 1.7 * x + 0.1 * rng.next(), x)
            })
            .collect();
        let run = |n: usize| {
            let mut f = StateSpaceFilter::hedge("usd", 1e-4, 0.01, 1.0).unwrap();
            let (mut x, mut p) = (vec![0.0; 2], vec![1.0, 0.0, 0.0, 1.0]);
            let mut bits = Vec::new();
            for (y, h) in &pairs[..n] {
                let step = f.observe_pair(&[*y], &[*h], "usd").unwrap();
                check_step(&step, &oracle(&mut x, &mut p, 1e-4, 0.01, &[1.0, *h], *y));
                bits.push(step.mean.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
            }
            bits
        };
        let whole = run(60);
        assert_eq!(run(20), whole[..20]);
        let mut f = StateSpaceFilter::hedge("usd", 1e-4, 0.01, 1.0).unwrap();
        let before = f.clone();
        assert_eq!(
            f.observe_pair(&[1.0, 2.0], &[1.0], "usd").unwrap_err(),
            FilterRefusal::DimensionMismatch
        );
        assert_eq!(
            f.observe_pair(&[1.0], &[], "usd").unwrap_err(),
            FilterRefusal::DimensionMismatch
        );
        assert_eq!(
            f.observe_pair(&[1.0], &[1.0], "eur").unwrap_err(),
            FilterRefusal::UnitMismatch
        );
        assert_eq!(f, before);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6.5
    #[test]
    fn factor_profile_oracle() {
        let order: Vec<String> = ["mkt", "smb", "hml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut rng = Lcg(17);
        let rows: Vec<(f64, Vec<f64>)> = (0..50)
            .map(|_| {
                let fs: Vec<f64> = (0..3).map(|_| rng.next()).collect();
                (0.5 * fs[0] - 0.2 * fs[1] + 0.05 * rng.next(), fs)
            })
            .collect();
        let run = |n: usize| {
            let mut f = StateSpaceFilter::factor(order.clone(), 1e-4, 0.01, 1.0).unwrap();
            let mut x = vec![0.0; 3];
            let mut p = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
            let mut bits = Vec::new();
            for (t, (y, fs)) in rows[..n].iter().enumerate() {
                let known = vec![t as i64; 3];
                let step = f.observe_factors(*y, t as i64, &order, fs, &known).unwrap();
                check_step(&step, &oracle(&mut x, &mut p, 1e-4, 0.01, fs, *y));
                bits.push(step.mean.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
            }
            bits
        };
        let whole = run(50);
        assert_eq!(run(25), whole[..25]);
        let mut f = StateSpaceFilter::factor(order.clone(), 1e-4, 0.01, 1.0).unwrap();
        let before = f.clone();
        let fs = [0.1, 0.2, 0.3];
        assert_eq!(
            f.observe_factors(1.0, 5, &order, &fs[..2], &[1, 1])
                .unwrap_err(),
            FilterRefusal::DimensionMismatch
        );
        assert_eq!(
            f.observe_factors(1.0, 5, &order, &fs, &[1, 1]).unwrap_err(),
            FilterRefusal::DimensionMismatch
        );
        let swapped: Vec<String> = ["smb", "mkt", "hml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            f.observe_factors(1.0, 5, &swapped, &fs, &[1, 1, 1])
                .unwrap_err(),
            FilterRefusal::FactorOrder
        );
        assert_eq!(
            f.observe_factors(1.0, 5, &order, &fs, &[1, 6, 1])
                .unwrap_err(),
            FilterRefusal::FutureLeak
        );
        assert_eq!(f, before);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.6
    #[test]
    fn four_models_match_independent_oracles() {
        // Every profile above is checked step by step against the oracle and its prefix;
        // this closes the rollup on the shared registry codes each refusal carries.
        use medulla_types::contract::ServerErrorCode as C;
        for refusal in [
            FilterRefusal::NoiseCorrelated,
            FilterRefusal::TransformInvalid,
            FilterRefusal::UnitMismatch,
            FilterRefusal::FactorOrder,
            FilterRefusal::FutureLeak,
            FilterRefusal::Warmup,
            FilterRefusal::RetuneBudget,
            FilterRefusal::WindowInvalid,
            FilterRefusal::ModelUnregistered,
            FilterRefusal::JacobianInvalid,
            FilterRefusal::DomainViolation,
            FilterRefusal::DimensionMismatch,
        ] {
            assert!(
                C::ALL.iter().any(|c| c.as_str() == refusal.code()),
                "{refusal:?}"
            );
        }
        causal_linear_recurrence_and_noise_refusal();
        volatility_profile_oracle();
        mean_reversion_profile_oracle();
        hedge_profile_oracle();
        factor_profile_oracle();
    }

    /// The independent scalar statistic of a level filter (`F = 1`, `q = 0`, `H = 1`).
    fn scalar_stats(obs: &[f64], r: f64, p0: f64) -> Vec<f64> {
        let (mut x, mut p) = (0.0, p0);
        obs.iter()
            .map(|z| {
                let s = p + r;
                let v = z - x;
                let k = p / s;
                x += k * v;
                p *= 1.0 - k;
                (v / s.sqrt()).clamp(-8.0, 8.0).powi(2)
            })
            .collect()
    }

    /// The schedule of R002.7 restated on the scalar statistics: indices of confirmed shifts.
    fn expected_shifts(stats: &[f64]) -> Vec<(u64, f64)> {
        let (mut shifts, mut consec) = (Vec::new(), 0);
        let mut baseline: Vec<f64> = stats[..32].to_vec();
        let mut cooldown_until = 0usize;
        for count in 64..=stats.len() {
            if count == cooldown_until {
                baseline = stats[count - 32..count].to_vec();
                consec = 0;
                continue;
            }
            if count < cooldown_until || count % 32 != 0 {
                continue;
            }
            let recent = &stats[count - 32..count];
            let ratio = (recent.iter().sum::<f64>() / 32.0)
                / (baseline.iter().sum::<f64>() / 32.0).max(1e-12);
            if (0.5..=2.0).contains(&ratio) {
                consec = 0;
                continue;
            }
            consec += 1;
            if consec == 3 {
                shifts.push((count as u64, ratio));
                consec = 0;
                cooldown_until = count + 64;
            }
        }
        shifts
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.7
    #[test]
    fn bounded_innovation_retuning() {
        let build = || {
            StateSpaceFilter::new(
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
            .with_shift_detector(ShiftConfig::default())
            .unwrap()
        };
        let drive = |obs: &[f64]| -> (Vec<(u64, f64, f64)>, Vec<ShiftOutcome>, StateSpaceFilter) {
            let mut f = build();
            let (mut shifts, mut outcomes) = (Vec::new(), Vec::new());
            for (i, z) in obs.iter().enumerate() {
                let before = f.measurement_noise()[0];
                let step = f.update(&[1.0], &[*z], None).unwrap();
                // The retune of a previous step has already applied: S carries the new R.
                assert!(step.innovation_covariance[0] >= before);
                match f.detect_shift() {
                    Ok(ShiftOutcome::Shifted(rep)) => {
                        assert_eq!(rep.cursor, i as u64 + 1);
                        assert_ne!(rep.old_digest, rep.new_digest);
                        shifts.push((rep.cursor, rep.ratio, rep.scale));
                        outcomes.push(ShiftOutcome::Shifted(rep));
                    }
                    Ok(other) => outcomes.push(other),
                    Err(e) => {
                        assert_eq!(e, FilterRefusal::Warmup);
                        assert!(i < 63, "warmup refuses only before 64 observations");
                    }
                }
            }
            (shifts, outcomes, f)
        };
        let mut rng = Lcg(23);
        // Stable: no shift, ever.
        let stable: Vec<f64> = (0..400).map(|_| rng.next()).collect();
        assert!(drive(&stable).0.is_empty());
        assert!(expected_shifts(&scalar_stats(&stable, 1.0, 1.0)).is_empty());
        // Shifted: variance x25 from observation 101.
        let shifted: Vec<f64> = (0..400)
            .map(|i| rng.next() * if i < 100 { 1.0 } else { 5.0 })
            .collect();
        let want = expected_shifts(&scalar_stats(&shifted, 1.0, 1.0));
        let (got, _, filter) = drive(&shifted);
        assert!(!want.is_empty(), "the oracle sees the shift");
        // The filter's own S moves with each retune, so compare the trigger cursor and clamp.
        assert_eq!(got[0].0, want[0].0, "exact trigger index");
        assert!(got
            .iter()
            .all(|(_, _, scale)| *scale <= 16.0 && *scale >= 1.0 / 16.0));
        assert!(
            (got[0].2 - 2.0).abs() < 1e-12,
            "the first retune scales R by the clamped ratio 2.0"
        );
        assert!(
            filter.measurement_noise()[0] >= 2.0,
            "revision applies from the next observation"
        );
        assert_eq!(
            filter.initial_noise_digest(),
            build().initial_noise_digest()
        );
        // Cooldown: the 64 observations after the trigger report Cooldown, then it re-arms.
        let (_, outcomes, _) = drive(&shifted);
        let first = outcomes
            .iter()
            .position(|o| matches!(o, ShiftOutcome::Shifted(_)))
            .unwrap();
        assert!(outcomes[first + 1..first + 60]
            .iter()
            .all(|o| *o == ShiftOutcome::Cooldown));
        // An isolated outlier never confirms: pending once, then stable again.
        let mut outlier: Vec<f64> = (0..320).map(|_| rng.next()).collect();
        outlier[199] = 40.0;
        let (shifts, outcomes, _) = drive(&outlier);
        assert!(shifts.is_empty());
        assert!(outcomes.contains(&ShiftOutcome::Pending(1)));
        assert!(!outcomes.contains(&ShiftOutcome::Pending(2)));
        // Warmup refuses before 64 observations.
        let mut young = build();
        young.update(&[1.0], &[1.0], None).unwrap();
        assert_eq!(young.detect_shift().unwrap_err(), FilterRefusal::Warmup);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.8
    #[test]
    fn nonlinear_robust_gain() {
        let model = NonlinearModel::from_id("squared_observation").unwrap();
        assert_eq!(
            NonlinearModel::from_id("nope").unwrap_err(),
            FilterRefusal::ModelUnregistered
        );
        let fresh = |x0: f64| {
            StateSpaceFilter::new(
                1,
                1,
                vec![1.0],
                vec![0.01],
                vec![0.04],
                vec![x0],
                vec![0.5],
                SYMMETRY_TOL,
            )
            .unwrap()
        };
        // Independent oracle for the extended update at the prior state with the cap c = 2.5.
        let oracle_step = |x: f64, p: f64, z: f64| {
            let (xm, pm) = (x, p + 0.01);
            let h = 2.0 * xm;
            let s = h * pm * h + 0.04;
            let v = z - xm * xm;
            let d = (v * v / s).sqrt();
            let w = (2.5 / d.max(1e-12)).min(1.0);
            let k = w * pm * h / s;
            let xn = xm + k * v;
            let pn = (1.0 - k * h) * (1.0 - k * h) * pm + k * 0.04 * k;
            (xn, pn, w, d)
        };
        // Both residual signs, a seeded heavy tail, and the small-residual (w = 1) regime.
        let mut rng = Lcg(29);
        let mut f = fresh(1.0);
        let (mut x, mut p) = (1.0, 0.5);
        let mut weights = Vec::new();
        for i in 0..40 {
            let tail = if i % 9 == 4 {
                30.0 * rng.next().signum() * (1.0 + rng.next().abs())
            } else {
                0.0
            };
            let z = x * x + 0.1 * rng.next() + tail;
            let step = f.update_robust(model, z).unwrap();
            let (xn, pn, w, d) = oracle_step(x, p, z);
            close(step.mean[0], xn);
            close(step.covariance[0], pn);
            close(step.weight, w);
            close(step.distance, d);
            assert!(step.approximation);
            weights.push(step.weight);
            (x, p) = (xn, pn);
        }
        assert!(weights.iter().any(|w| *w < 1.0) && weights.iter().any(|w| *w == 1.0));
        // Bounded correction: the heavy tail moves the state no more than the capped gain allows.
        let mut tail = fresh(1.0);
        let bounded = tail.update_robust(model, 1.0 + 1e6).unwrap();
        assert!(
            (bounded.mean[0] - 1.0).abs() < 2.0,
            "capped, not proportional to 1e6"
        );
        // Refusals keep the state.
        let mut f = fresh(-1.0);
        let before = f.clone();
        assert_eq!(
            f.update_robust(model, 1.0).unwrap_err(),
            FilterRefusal::DomainViolation
        );
        assert_eq!(
            f.update_robust(model, f64::NAN).unwrap_err(),
            FilterRefusal::InputNonfinite
        );
        assert_eq!(f, before);
        let overflow = StateSpaceFilter::new(
            1,
            1,
            vec![1.0],
            vec![0.0],
            vec![1.0],
            vec![1e200],
            vec![1.0],
            SYMMETRY_TOL,
        )
        .unwrap()
        .update_robust(model, 1.0);
        assert_eq!(overflow.unwrap_err(), FilterRefusal::JacobianInvalid);
    }
}
