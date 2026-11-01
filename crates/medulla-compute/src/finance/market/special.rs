//! Special functions (self-contained — no scipy on the wire).
// The normal CDF/quantile and the sample moments moved to the evaluation kernels
// (`medulla_numeric::evaluation::special`, EH-530) with the backtest validation that used
// them; the Beta and Gauss-Legendre functions below serve the Kelly sizing here.

/// ln Γ(x) — Lanczos approximation (g = 7, n = 9).
pub fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // reflection
        std::f64::consts::PI.ln() - (std::f64::consts::PI * x).sin().ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let mut a = C[0];
        let t = x + G + 0.5;
        for (i, &c) in C.iter().enumerate().skip(1) {
            a += c / (x + i as f64);
        }
        0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

fn ln_beta(a: f64, b: f64) -> f64 {
    ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
}

/// Beta(a,b) pdf at x ∈ (0,1).
pub fn beta_pdf(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 || x >= 1.0 {
        return 0.0;
    }
    ((a - 1.0) * x.ln() + (b - 1.0) * (1.0 - x).ln() - ln_beta(a, b)).exp()
}

/// Continued fraction for the incomplete beta (Numerical Recipes betacf).
fn betacf(a: f64, b: f64, x: f64) -> f64 {
    let fpmin = 1e-30;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < fpmin {
        d = fpmin;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..200 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < fpmin {
            d = fpmin;
        }
        c = 1.0 + aa / c;
        if c.abs() < fpmin {
            c = fpmin;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < fpmin {
            d = fpmin;
        }
        c = 1.0 + aa / c;
        if c.abs() < fpmin {
            c = fpmin;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-12 {
            break;
        }
    }
    h
}

/// Regularized incomplete beta I_x(a,b) = Beta CDF.
pub fn betai(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    // exact log-prefactor x^a (1-x)^b / (a·B(a,b))
    let lbt = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    let pref = lbt.exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        pref * betacf(a, b, x) / a
    } else {
        1.0 - pref * betacf(b, a, 1.0 - x) / b
    }
}

/// Inverse Beta CDF (quantile) via bisection on betai.
pub fn beta_ppf(p: f64, a: f64, b: f64) -> f64 {
    if p <= 0.0 {
        return 0.0;
    }
    if p >= 1.0 {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if betai(a, b, mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Gauss-Legendre nodes & weights on [-1, 1] (Newton on Legendre roots).
pub fn leggauss(n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut x = vec![0.0; n];
    let mut w = vec![0.0; n];
    let m = n.div_ceil(2);
    for i in 0..m {
        // initial guess
        let mut z = (std::f64::consts::PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        let mut z1;
        let mut pp;
        loop {
            let mut p1 = 1.0;
            let mut p2 = 0.0;
            for j in 0..n {
                let p3 = p2;
                p2 = p1;
                p1 = ((2.0 * j as f64 + 1.0) * z * p2 - j as f64 * p3) / (j as f64 + 1.0);
            }
            pp = n as f64 * (z * p1 - p2) / (z * z - 1.0);
            z1 = z;
            z = z1 - p1 / pp;
            if (z - z1).abs() < 1e-14 {
                break;
            }
        }
        x[i] = -z;
        x[n - 1 - i] = z;
        let wi = 2.0 / ((1.0 - z * z) * pp * pp);
        w[i] = wi;
        w[n - 1 - i] = wi;
    }
    (x, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_special_functions() {
        // Beta(2,2) is symmetric: median = 0.5, cdf(0.5)=0.5
        assert!((betai(2.0, 2.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((beta_ppf(0.5, 2.0, 2.0) - 0.5).abs() < 1e-4);
    }
}
