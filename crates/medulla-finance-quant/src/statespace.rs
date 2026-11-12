// CONCEPT:MEDULLA-KG.domains.state-space-statistical-arbitrage — State-Space & Statistical-Arbitrage Kernels
//
// The Kalman, ADF and Ornstein-Uhlenbeck kernels live in `medulla_numeric::evaluation::statarb`
// (MEDULLA-FINANCE-PRIMITIVES-R002.1.11..R002.1.13): one implementation behind the finance
// Methods and `NumericEvaluate`. This module re-exports them and keeps the Markov
// transition-matrix estimator.

pub use medulla_numeric::evaluation::statarb::{
    adf_test, kalman_beta, kalman_filter_1d, kalman_volatility, ou_calibrate, ou_optimal_thresholds,
};
pub use medulla_types::numeric_kernel::{AdfResult, KalmanState, OuParams, OuThresholds};

// ════════════════════════════════════════════════════════════════════════
//  Markov transition matrix (cross-venue regime / lead-lag)
// ════════════════════════════════════════════════════════════════════════

/// Estimate an `n_states`×`n_states` row-stochastic transition matrix from a
/// sequence of integer states (Laplace-smoothed). Used for cross-venue lead-lag
/// ("does an imbalance shift on A predict a book clear on B?").
pub fn markov_transition_matrix(states: &[usize], n_states: usize) -> Vec<Vec<f64>> {
    if n_states == 0 {
        return vec![];
    }
    let mut counts = vec![vec![1.0_f64; n_states]; n_states]; // Laplace prior
    for w in states.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a < n_states && b < n_states {
            counts[a][b] += 1.0;
        }
    }
    counts
        .into_iter()
        .map(|row| {
            let total: f64 = row.iter().sum();
            row.into_iter().map(|c| c / total).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kalman_filter_1d_tracks_constant() {
        // noisy observations of a constant 5.0; filter should converge near 5
        let obs: Vec<f64> = (0..100)
            .map(|i| 5.0 + 0.01 * ((i % 5) as f64 - 2.0))
            .collect();
        let out = kalman_filter_1d(&obs, 1.0, 1e-5, 1.0, 1e-2, 0.0, 1.0);
        assert!((out.states.last().unwrap() - 5.0).abs() < 0.1);
    }

    #[test]
    fn test_kalman_beta_recovers_known_beta() {
        // r_asset = 1.5 * r_market + small noise; filter should land near 1.5
        let rm: Vec<f64> = (0..300).map(|i| ((i as f64 * 0.1).sin()) * 0.01).collect();
        let ra: Vec<f64> = rm
            .iter()
            .enumerate()
            .map(|(i, m)| 1.5 * m + 1e-5 * ((i % 3) as f64 - 1.0))
            .collect();
        let out = kalman_beta(&rm, &ra, 1e-6, 1e-4, 1.0, 1.0);
        assert!(
            (out.states.last().unwrap() - 1.5).abs() < 0.2,
            "beta={}",
            out.states.last().unwrap()
        );
    }

    #[test]
    fn test_kalman_volatility_positive_and_reasonable() {
        let rets: Vec<f64> = (0..250).map(|i| 0.01 * ((i as f64 * 0.3).sin())).collect();
        let vol = kalman_volatility(&rets, 0.1, 1.0, None, 1.0, 252.0);
        assert_eq!(vol.len(), 250);
        assert!(vol.iter().all(|v| *v >= 0.0 && v.is_finite()));
    }

    #[test]
    fn test_adf_stationary_vs_random_walk() {
        // Deterministic LCG noise so partial sums genuinely wander (a periodic
        // increment would make the "random walk" bounded ⇒ falsely stationary).
        let mut seed = 12_345_u64;
        let mut noise = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        };
        // stationary AR(1): x_t = 0.2 x_{t-1} + e
        let mut x = vec![0.0];
        for i in 1..400 {
            x.push(0.2 * x[i - 1] + noise());
        }
        // genuine random walk: y_t = y_{t-1} + e (unit root)
        let mut y = vec![0.0];
        for _ in 1..400 {
            y.push(y.last().unwrap() + noise());
        }
        let stat = adf_test(&x, 1);
        let rw = adf_test(&y, 1);
        // stationary series rejects the unit root (very negative ADF stat); the
        // random walk does not — so the stationary stat is more negative.
        assert!(
            stat.statistic < rw.statistic,
            "stat={} rw={}",
            stat.statistic,
            rw.statistic
        );
        assert!(
            stat.stationary_5pct,
            "AR(0.2) should be stationary: {}",
            stat.statistic
        );
        assert!(
            !rw.stationary_5pct,
            "random walk should NOT be stationary: {}",
            rw.statistic
        );
        // interpolated criticals are ordered 1% < 5% < 10% (more negative = stricter)
        assert!(stat.crit_1pct < stat.crit_5pct && stat.crit_5pct < stat.crit_10pct);
        // p-value is in [0,1] and the strongly-stationary series has the smaller p
        assert!((0.0..=1.0).contains(&stat.p_value_approx));
        assert!((0.0..=1.0).contains(&rw.p_value_approx));
        assert!(
            stat.p_value_approx < rw.p_value_approx,
            "stationary p {} should be < RW p {}",
            stat.p_value_approx,
            rw.p_value_approx
        );
    }

    #[test]
    fn test_ou_calibrate_recovers_reversion() {
        // simulate OU around mu=0.5 with strong reversion
        let mut s = vec![0.5];
        for i in 1..500 {
            let prev = s[i - 1];
            let drift = 0.3 * (0.5 - prev);
            s.push(prev + drift + 0.01 * ((i % 11) as f64 - 5.0));
        }
        let p = ou_calibrate(&s, 1.0);
        assert!(p.theta > 0.0, "theta={}", p.theta);
        assert!((p.mu - 0.5).abs() < 0.2, "mu={}", p.mu);
        assert!(p.half_life > 0.0 && p.half_life.is_finite());
        assert!(p.sigma_eq >= 0.0);
    }

    #[test]
    fn test_ou_optimal_thresholds_band_brackets_mean() {
        let p = OuParams {
            theta: 0.5,
            mu: 0.0,
            sigma: 0.1,
            half_life: 1.386,
            sigma_eq: 0.1,
        };
        let th = ou_optimal_thresholds(&p, 0.001);
        assert!(th.entry_long < th.exit && th.exit < th.entry_short);
        assert!(th.z > 0.0);
        assert!(th.expected_return_per_unit_time >= 0.0);
    }

    #[test]
    fn test_markov_transition_rows_sum_to_one() {
        let states = vec![0, 1, 1, 2, 0, 1, 2, 2, 0];
        let m = markov_transition_matrix(&states, 3);
        assert_eq!(m.len(), 3);
        for row in &m {
            let s: f64 = row.iter().sum();
            assert!((s - 1.0).abs() < 1e-9);
        }
    }
}
