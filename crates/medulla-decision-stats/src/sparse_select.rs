//! Joint sparse nonnegative reconstruction (MEDULLA-FEDERATED-QUERY-R081.100).
//!
//! Given a query vector `q` and candidate rows `V`, minimise
//! `||q - Vᵀx||₂² + λ||x||₁` over `x ≥ 0` by cyclic coordinate descent. The one-term
//! update is `x_i = max(0, (v_i·r_i - λ/2) / ||v_i||²)` where `r_i` is the residual with
//! term `i` removed. The kernel is a pure `f64` function: it reads no store, calls no
//! model and holds no state, so the same input always yields the same solution.

/// Why a solve refused.
#[derive(Debug, Clone, PartialEq)]
pub enum SparseSelectError {
    /// The query or a candidate is empty, has a different length, or holds a non-finite value.
    Shape(String),
    /// `lambda` is not a positive finite number, `tolerance` is not positive, or no sweep is allowed.
    Parameter(String),
    /// The largest coefficient change did not fall below `tolerance` within `max_iters` sweeps.
    NotConverged {
        /// Sweeps run.
        iterations: u32,
        /// The largest coefficient change of the last sweep.
        max_delta: f64,
    },
}

impl std::fmt::Display for SparseSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(detail) | Self::Parameter(detail) => f.write_str(detail),
            Self::NotConverged {
                iterations,
                max_delta,
            } => write!(
                f,
                "no convergence after {iterations} sweeps (largest change {max_delta})"
            ),
        }
    }
}

impl std::error::Error for SparseSelectError {}

/// A converged solution.
#[derive(Debug, Clone, PartialEq)]
pub struct SparseSolution {
    /// One coefficient per candidate row, all `>= 0`, in input order.
    pub coefficients: Vec<f64>,
    /// `||q - Vᵀx||₂`, recomputed from the final coefficients.
    pub residual: f64,
    /// Sweeps run.
    pub iterations: u32,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Solve the joint sparse nonnegative reconstruction of `query` from `rows`.
pub fn joint_sparse_select(
    query: &[f64],
    rows: &[Vec<f64>],
    lambda: f64,
    tolerance: f64,
    max_iters: u32,
) -> Result<SparseSolution, SparseSelectError> {
    if !(lambda.is_finite() && lambda > 0.0) {
        return Err(SparseSelectError::Parameter(
            "lambda must be a positive finite number".into(),
        ));
    }
    if !(tolerance.is_finite() && tolerance > 0.0) || max_iters == 0 {
        return Err(SparseSelectError::Parameter(
            "tolerance must be positive and at least one sweep is required".into(),
        ));
    }
    if query.is_empty() || query.iter().any(|v| !v.is_finite()) {
        return Err(SparseSelectError::Shape(
            "the query vector must be non-empty and finite".into(),
        ));
    }
    if rows
        .iter()
        .any(|r| r.len() != query.len() || r.iter().any(|v| !v.is_finite()))
    {
        return Err(SparseSelectError::Shape(
            "every candidate must be finite and as long as the query".into(),
        ));
    }
    let norms: Vec<f64> = rows.iter().map(|r| dot(r, r)).collect();
    let mut x = vec![0.0_f64; rows.len()];
    let mut residual: Vec<f64> = query.to_vec();
    let mut max_delta = f64::INFINITY;
    for sweep in 1..=max_iters {
        max_delta = 0.0;
        for (i, row) in rows.iter().enumerate() {
            if norms[i] == 0.0 {
                continue;
            }
            // v_i . r_i, with r_i = r + x_i v_i.
            let rho = dot(row, &residual) + norms[i] * x[i];
            let next = ((rho - lambda / 2.0) / norms[i]).max(0.0);
            let delta = next - x[i];
            if delta != 0.0 {
                for (r, v) in residual.iter_mut().zip(row) {
                    *r -= delta * v;
                }
                x[i] = next;
            }
            max_delta = max_delta.max(delta.abs());
        }
        if max_delta < tolerance {
            let mut exact = query.to_vec();
            for (coefficient, row) in x.iter().zip(rows) {
                if *coefficient != 0.0 {
                    for (r, v) in exact.iter_mut().zip(row) {
                        *r -= coefficient * v;
                    }
                }
            }
            return Ok(SparseSolution {
                coefficients: x,
                residual: dot(&exact, &exact).sqrt(),
                iterations: sweep,
            });
        }
    }
    Err(SparseSelectError::NotConverged {
        iterations: max_iters,
        max_delta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solve `a x = b` by Gaussian elimination with partial pivoting; `None` when singular.
    fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
        let n = b.len();
        for col in 0..n {
            let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
            if a[pivot][col].abs() < 1e-12 {
                return None;
            }
            a.swap(col, pivot);
            b.swap(col, pivot);
            for row in col + 1..n {
                let factor = a[row][col] / a[col][col];
                for k in col..n {
                    a[row][k] -= factor * a[col][k];
                }
                b[row] -= factor * b[col];
            }
        }
        let mut x = vec![0.0; n];
        for row in (0..n).rev() {
            let tail: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
            x[row] = (b[row] - tail) / a[row][row];
        }
        Some(x)
    }

    /// The independent oracle: try every support, solve its normal equations with the
    /// `λ/2` shrinkage, and keep the one that satisfies the KKT conditions of the
    /// nonnegative lasso (positive on the support, `v_j·r - λ/2 <= 0` off it).
    fn exhaustive_oracle(q: &[f64], rows: &[Vec<f64>], lambda: f64) -> Vec<f64> {
        let n = rows.len();
        for mask in 0..(1_u32 << n) {
            let support: Vec<usize> = (0..n).filter(|i| mask >> i & 1 == 1).collect();
            let gram: Vec<Vec<f64>> = support
                .iter()
                .map(|&i| support.iter().map(|&j| dot(&rows[i], &rows[j])).collect())
                .collect();
            let rhs: Vec<f64> = support
                .iter()
                .map(|&i| dot(&rows[i], q) - lambda / 2.0)
                .collect();
            let Some(on) = (if support.is_empty() {
                Some(Vec::new())
            } else {
                solve(gram, rhs)
            }) else {
                continue;
            };
            if on.iter().any(|v| *v <= 1e-12) {
                continue;
            }
            let mut x = vec![0.0; n];
            for (&i, v) in support.iter().zip(&on) {
                x[i] = *v;
            }
            let mut r = q.to_vec();
            for (coefficient, row) in x.iter().zip(rows) {
                for (rv, v) in r.iter_mut().zip(row) {
                    *rv -= coefficient * v;
                }
            }
            if (0..n)
                .filter(|j| x[*j] == 0.0)
                .all(|j| dot(&rows[j], &r) - lambda / 2.0 <= 1e-9)
            {
                return x;
            }
        }
        panic!("the oracle found no KKT point");
    }

    fn fixture() -> (Vec<f64>, Vec<Vec<f64>>) {
        (
            vec![1.0, 0.8, 0.0],
            vec![
                vec![1.0, 0.0, 0.0],
                vec![0.0, 1.0, 0.0],
                vec![0.4, 0.4, 0.8],
                vec![0.38, 0.42, 0.81],
            ],
        )
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.100
    #[test]
    fn matches_exhaustive_oracle_on_tiny_fixture() {
        let (q, rows) = fixture();
        for lambda in [0.01, 0.1, 0.5] {
            let solved = joint_sparse_select(&q, &rows, lambda, 1e-12, 100_000).expect("converges");
            let oracle = exhaustive_oracle(&q, &rows, lambda);
            for (got, want) in solved.coefficients.iter().zip(&oracle) {
                assert!(
                    (got - want).abs() < 1e-6,
                    "lambda {lambda}: {got} vs {want}"
                );
            }
            assert!(solved.coefficients.iter().all(|c| *c >= 0.0));
        }
        // The diverse pair is selected, the correlated pair is not.
        let solved = joint_sparse_select(&q, &rows, 0.1, 1e-12, 100_000).expect("converges");
        assert!((solved.coefficients[0] - 0.95).abs() < 1e-6);
        assert!((solved.coefficients[1] - 0.75).abs() < 1e-6);
        assert_eq!(solved.coefficients[2], 0.0);
        assert_eq!(solved.coefficients[3], 0.0);
        assert!((solved.residual - 0.05_f64.hypot(0.05)).abs() < 1e-6);
        // Deterministic: the same input solves identically.
        assert_eq!(
            solved,
            joint_sparse_select(&q, &rows, 0.1, 1e-12, 100_000).unwrap()
        );
    }

    #[test]
    fn refuses_bad_parameters_shapes_and_non_convergence() {
        let (q, rows) = fixture();
        for lambda in [0.0, -1.0, f64::NAN] {
            assert!(matches!(
                joint_sparse_select(&q, &rows, lambda, 1e-6, 10),
                Err(SparseSelectError::Parameter(_))
            ));
        }
        assert!(matches!(
            joint_sparse_select(&q, &rows, 0.1, 0.0, 10),
            Err(SparseSelectError::Parameter(_))
        ));
        assert!(matches!(
            joint_sparse_select(&q, &[vec![1.0]], 0.1, 1e-6, 10),
            Err(SparseSelectError::Shape(_))
        ));
        assert!(matches!(
            joint_sparse_select(&[f64::NAN, 0.0, 0.0], &rows, 0.1, 1e-6, 10),
            Err(SparseSelectError::Shape(_))
        ));
        // One sweep cannot settle a coupled problem to a 1e-15 tolerance.
        assert!(matches!(
            joint_sparse_select(&q, &rows, 0.1, 1e-15, 1),
            Err(SparseSelectError::NotConverged { iterations: 1, .. })
        ));
        // A zero candidate row is skipped and keeps a zero coefficient.
        let mut with_zero = rows.clone();
        with_zero.push(vec![0.0; 3]);
        let solved = joint_sparse_select(&q, &with_zero, 0.1, 1e-12, 100_000).unwrap();
        assert_eq!(solved.coefficients[4], 0.0);
    }
}
