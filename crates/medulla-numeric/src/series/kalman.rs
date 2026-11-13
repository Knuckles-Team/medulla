//! The scalar Kalman filter (EH-530, re-homed from the finance state-space module): one
//! recurrence behind the local-level and dynamic-beta series kernels and the finance
//! `kalman_filter_1d` / `kalman_beta` Methods.

use serde::{Deserialize, Serialize};

use super::KalmanNoise;

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

#[cfg(all(test, feature = "evaluate"))]
mod tests {
    use super::*;
    use crate::evaluation::registry::evaluate;
    use medulla_types::numeric_kernel::{KernelCode, NumericEvaluateOp as Op, NumericKernelOutput};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} != {b}");
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.11
    #[test]
    fn kalman_independent_vectors() {
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
}
