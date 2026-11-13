use super::special;

// ════════════════════════════════════════════════════════════════════════
//  Position sizing: Kelly & Bayesian Kelly
// ════════════════════════════════════════════════════════════════════════

/// Fractional Kelly for a YES contract priced at c with true-prob estimate q:
///   f* = (q − c) / (1 − c), scaled by `fraction` and floored at 0.
pub fn kelly_fraction(q: f64, c: f64, fraction: f64) -> f64 {
    if c >= q || c <= 0.0 || c >= 1.0 {
        return 0.0;
    }
    let f_star = (q - c) / (1.0 - c);
    (f_star * fraction).clamp(0.0, 1.0)
}

/// Bayesian Kelly under a Beta(α,β) posterior over the true probability.
/// Maximises E_q[U(f)] via Gauss-Legendre quadrature over q and a grid on f.
pub fn bayesian_kelly_fraction(alpha: f64, beta: f64, c: f64, n_quadrature: usize) -> f64 {
    if c <= 0.0 || c >= 1.0 {
        return 0.0;
    }
    let (nodes, weights) = special::leggauss(n_quadrature.max(8));
    // map [-1,1] -> [0,1]
    let q_grid: Vec<f64> = nodes.iter().map(|&z| 0.5 * (z + 1.0)).collect();
    let q_w: Vec<f64> = weights
        .iter()
        .zip(&q_grid)
        .map(|(&w, &q)| 0.5 * w * special::beta_pdf(q, alpha, beta))
        .collect();
    let b = (1.0 - c) / c;
    let neg_eu = |f: f64| -> f64 {
        if f <= 0.0 || f >= 1.0 {
            return 1e10;
        }
        let mut acc = 0.0;
        for (i, &q) in q_grid.iter().enumerate() {
            let u = (1.0 - q) * (1.0 - f).ln() + q * (1.0 + f * b).ln();
            acc += q_w[i] * u;
        }
        -acc
    };
    let mut best_f = 0.0;
    let mut best_v = f64::INFINITY;
    let steps = 200;
    for i in 1..steps {
        let f = i as f64 / steps as f64;
        let v = neg_eu(f);
        if v < best_v {
            best_v = v;
            best_f = f;
        }
    }
    best_f.max(0.0)
}

/// Equal-tailed credible interval for q ~ Beta(α,β); use the lower bound as a
/// conservative Kelly input.
pub fn posterior_credible_interval(alpha: f64, beta: f64, level: f64) -> (f64, f64) {
    (
        special::beta_ppf(level / 2.0, alpha, beta),
        special::beta_ppf(1.0 - level / 2.0, alpha, beta),
    )
}

// ════════════════════════════════════════════════════════════════════════
//  Backtest validation: purged CPCV, Deflated Sharpe, PBO, Diebold-Mariano
// ════════════════════════════════════════════════════════════════════════
