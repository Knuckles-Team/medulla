// CONCEPT:MEDULLA-KG.compute.ml-primitives — ML Primitives
//
// Core machine learning building blocks implemented in Rust.
// These replace scipy/sklearn hot paths and are served over the Tokio
// service protocol (no in-process Python extension).

pub use medulla_result_types::compute_result::datascience::RegressionResult;

use medulla_numeric::cluster::{fit_simple, ClusterConfig, ClusterRefusal, Rows};
pub use medulla_result_types::compute_result::datascience::KMeansResult;

pub use medulla_result_types::compute_result::datascience::PCAResult;

pub use medulla_result_types::compute_result::datascience::DatasetStats;

// ── Ordinary Least Squares Regression ─────────────────────────────────

/// Fit OLS linear regression: y = X @ β + intercept
///
/// Uses the normal equation: β = (X'X)^(-1) X'y
pub fn linear_regression(x: &[Vec<f64>], y: &[f64]) -> RegressionResult {
    let n = x.len();
    let p = if n > 0 { x[0].len() } else { 0 };

    if n == 0 || p == 0 || n != y.len() {
        return RegressionResult {
            coefficients: vec![],
            intercept: 0.0,
            r_squared: 0.0,
            residuals: vec![],
        };
    }

    // Add intercept column (augmented X)
    let p_aug = p + 1;
    let mut xtx = vec![vec![0.0; p_aug]; p_aug];
    let mut xty = vec![0.0; p_aug];

    for i in 0..n {
        let mut row = vec![1.0]; // intercept
        row.extend_from_slice(&x[i]);

        for j in 0..p_aug {
            xty[j] += row[j] * y[i];
            for k in 0..p_aug {
                xtx[j][k] += row[j] * row[k];
            }
        }
    }

    // Solve via Cholesky-like approach (simple Gauss elimination)
    let beta = solve_linear_system(&xtx, &xty);

    let intercept = beta[0];
    let coefficients: Vec<f64> = beta[1..].to_vec();

    // Compute R-squared
    let y_mean: f64 = y.iter().sum::<f64>() / n as f64;
    let mut ss_res = 0.0;
    let mut ss_tot = 0.0;
    let mut residuals = Vec::with_capacity(n);

    for i in 0..n {
        let y_pred: f64 = intercept
            + coefficients
                .iter()
                .zip(x[i].iter())
                .map(|(c, xi)| c * xi)
                .sum::<f64>();
        let res = y[i] - y_pred;
        residuals.push(res);
        ss_res += res * res;
        ss_tot += (y[i] - y_mean) * (y[i] - y_mean);
    }

    let r_squared = if ss_tot > 0.0 {
        1.0 - ss_res / ss_tot
    } else {
        0.0
    };

    RegressionResult {
        coefficients,
        intercept,
        r_squared,
        residuals,
    }
}

// ── K-Means Clustering ───────────────────────────────────────────────

/// K-means clustering on the shared clustering kernel (MEDULLA-CLUSTER-R008.4).
/// `max_iter` is the step budget. Refuses `k = 0`, empty or ragged data, `k` above the
/// row count and non-finite values with a typed code instead of returning empty labels.
pub fn kmeans(
    data: &[Vec<f64>],
    k: usize,
    max_iter: usize,
) -> Result<KMeansResult, ClusterRefusal> {
    let n = data.len();
    let dim = data.first().map_or(0, Vec::len);
    if data.iter().any(|row| row.len() != dim) {
        return Err(ClusterRefusal::ShapeInvalid("rows have unequal width"));
    }
    let flat: Vec<f32> = data.iter().flatten().map(|&v| v as f32).collect();
    let cfg = ClusterConfig {
        max_steps: u32::try_from(max_iter.max(1)).unwrap_or(u32::MAX),
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&flat, n, dim), k, &cfg)?;
    let centroids = out
        .centroids
        .chunks(dim)
        .map(|c| c.iter().map(|&v| f64::from(v)).collect())
        .collect();
    Ok(KMeansResult {
        labels: out.labels.iter().map(|&l| l as usize).collect(),
        centroids,
        inertia: out.inertia,
        n_iterations: out.steps as usize,
    })
}

// ── PCA (Power Iteration Method) ─────────────────────────────────────

fn column_means(data: &[Vec<f64>], n: usize, dim: usize) -> Vec<f64> {
    (0..dim)
        .map(|d| data.iter().map(|row| row[d]).sum::<f64>() / n as f64)
        .collect()
}

fn center_row(row: &[f64], means: &[f64]) -> Vec<f64> {
    row.iter()
        .zip(means.iter())
        .map(|(value, mean)| value - mean)
        .collect()
}

fn centered_data(data: &[Vec<f64>], means: &[f64]) -> Vec<Vec<f64>> {
    data.iter().map(|row| center_row(row, means)).collect()
}

fn covariance_matrix(centered: &[Vec<f64>], n: usize, dim: usize) -> Vec<Vec<f64>> {
    let mut covariance = vec![vec![0.0; dim]; dim];
    for row in centered {
        for i in 0..dim {
            for j in i..dim {
                covariance[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 0..dim {
        for j in i..dim {
            covariance[i][j] /= (n - 1).max(1) as f64;
            covariance[j][i] = covariance[i][j];
        }
    }
    covariance
}

fn principal_components(
    covariance: &[Vec<f64>],
    n_components: usize,
    dim: usize,
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut components = Vec::with_capacity(n_components);
    let mut eigenvalues = Vec::with_capacity(n_components);
    let mut deflated_covariance = covariance.to_vec();
    for _ in 0..n_components {
        let (eigenvalue, eigenvector) = power_iteration(&deflated_covariance, 200);
        eigenvalues.push(eigenvalue);
        components.push(eigenvector.clone());
        for i in 0..dim {
            for j in 0..dim {
                deflated_covariance[i][j] -= eigenvalue * eigenvector[i] * eigenvector[j];
            }
        }
    }
    (components, eigenvalues)
}

fn explained_variance_ratios(eigenvalues: &[f64], total_variance: f64) -> Vec<f64> {
    eigenvalues
        .iter()
        .map(|&eigenvalue| {
            if total_variance > 0.0 {
                eigenvalue / total_variance
            } else {
                0.0
            }
        })
        .collect()
}

fn project_row(row: &[f64], components: &[Vec<f64>]) -> Vec<f64> {
    let mut projections = Vec::with_capacity(components.len());
    for component in components {
        let mut projection = 0.0;
        for (value, component_value) in row.iter().zip(component.iter()) {
            projection += value * component_value;
        }
        projections.push(projection);
    }
    projections
}

fn transformed_data(centered: &[Vec<f64>], components: &[Vec<f64>]) -> Vec<Vec<f64>> {
    centered
        .iter()
        .map(|row| project_row(row, components))
        .collect()
}

/// PCA via iterative eigendecomposition of the covariance matrix.
pub fn pca(data: &[Vec<f64>], n_components: usize) -> PCAResult {
    let n = data.len();
    let dim = if n > 0 { data[0].len() } else { 0 };

    if n == 0 || dim == 0 || n_components == 0 {
        return PCAResult {
            components: vec![],
            explained_variance: vec![],
            explained_variance_ratio: vec![],
            transformed: vec![],
        };
    }

    let n_comp = n_components.min(dim);

    let means = column_means(data, n, dim);
    let centered = centered_data(data, &means);
    let covariance = covariance_matrix(&centered, n, dim);
    let (components, eigenvalues) = principal_components(&covariance, n_comp, dim);

    let total_variance: f64 = eigenvalues.iter().sum();
    let explained_variance_ratio = explained_variance_ratios(&eigenvalues, total_variance);
    let transformed = transformed_data(&centered, &components);

    PCAResult {
        components,
        explained_variance: eigenvalues,
        explained_variance_ratio,
        transformed,
    }
}

/// Compute dataset statistics.
pub fn compute_stats(data: &[Vec<f64>]) -> DatasetStats {
    let n = data.len();
    let dim = if n > 0 { data[0].len() } else { 0 };

    if n == 0 || dim == 0 {
        return DatasetStats {
            means: vec![],
            stds: vec![],
            mins: vec![],
            maxs: vec![],
            correlation_matrix: vec![],
            n_samples: 0,
            n_features: 0,
        };
    }

    let means: Vec<f64> = (0..dim)
        .map(|d| data.iter().map(|row| row[d]).sum::<f64>() / n as f64)
        .collect();

    let stds: Vec<f64> = (0..dim)
        .map(|d| {
            let var: f64 = data
                .iter()
                .map(|row| (row[d] - means[d]).powi(2))
                .sum::<f64>()
                / (n - 1).max(1) as f64;
            var.sqrt()
        })
        .collect();

    let mins: Vec<f64> = (0..dim)
        .map(|d| data.iter().map(|row| row[d]).fold(f64::INFINITY, f64::min))
        .collect();

    let maxs: Vec<f64> = (0..dim)
        .map(|d| {
            data.iter()
                .map(|row| row[d])
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect();

    // Correlation matrix
    let mut corr = vec![vec![0.0; dim]; dim];
    for i in 0..dim {
        for j in 0..dim {
            if stds[i] > 1e-12 && stds[j] > 1e-12 {
                let cov: f64 = data
                    .iter()
                    .map(|row| (row[i] - means[i]) * (row[j] - means[j]))
                    .sum::<f64>()
                    / (n - 1).max(1) as f64;
                corr[i][j] = cov / (stds[i] * stds[j]);
            } else if i == j {
                corr[i][j] = 1.0;
            }
        }
    }

    DatasetStats {
        means,
        stds,
        mins,
        maxs,
        correlation_matrix: corr,
        n_samples: n,
        n_features: dim,
    }
}

/// Train-test split. When `shuffle` is true the rows are permuted with a
/// `seed`-seeded Fisher–Yates shuffle before splitting (matching scikit-learn's
/// `(x_train, x_test, y_train, y_test)` produced by [`train_test_split`].
pub type TrainTestSplit = (Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<f64>, Vec<f64>);

/// shuffle-by-default behavior and avoiding pure-extrapolation test folds on
/// ordered data); when false the split is deterministic and contiguous.
pub fn train_test_split(
    data: &[Vec<f64>],
    labels: &[f64],
    test_ratio: f64,
    shuffle: bool,
    seed: u64,
) -> TrainTestSplit {
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    let n = data.len();
    let split_idx = ((1.0 - test_ratio) * n as f64) as usize;

    let mut idx: Vec<usize> = (0..n).collect();
    if shuffle {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        for i in (1..n).rev() {
            let j = (rng.gen::<u64>() as usize) % (i + 1);
            idx.swap(i, j);
        }
    }

    let gather_x = |sel: &[usize]| sel.iter().map(|&i| data[i].clone()).collect();
    let gather_y = |sel: &[usize]| sel.iter().map(|&i| labels[i]).collect();

    let x_train = gather_x(&idx[..split_idx]);
    let x_test = gather_x(&idx[split_idx..]);
    let y_train = gather_y(&idx[..split_idx]);
    let y_test = gather_y(&idx[split_idx..]);

    (x_train, x_test, y_train, y_test)
}

// ── Internal Helpers ─────────────────────────────────────────────────

/// Solve Ax = b using Gaussian elimination with partial pivoting.
pub(crate) fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = a.len();
    let mut aug = augmented_matrix(a, b);
    forward_eliminate(&mut aug, n);
    back_substitute(&aug, n)
}

/// `[A | b]`, the augmented matrix Gaussian elimination operates on.
fn augmented_matrix(a: &[Vec<f64>], b: &[f64]) -> Vec<Vec<f64>> {
    a.iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(b[i]);
            r
        })
        .collect()
}

/// Forward elimination with partial pivoting, in place.
fn forward_eliminate(aug: &mut [Vec<f64>], n: usize) {
    for col in 0..n {
        let max_row = pivot_row(aug, col, n);
        aug.swap(col, max_row);

        let pivot = aug[col][col];
        if pivot.abs() < 1e-15 {
            continue;
        }
        eliminate_below_pivot(aug, col, n, pivot);
    }
}

/// The row in `col..n` with the largest-magnitude entry in column `col`
/// (partial pivoting).
fn pivot_row(aug: &[Vec<f64>], col: usize, n: usize) -> usize {
    let mut max_row = col;
    let mut max_val = aug[col][col].abs();
    for row in (col + 1)..n {
        if aug[row][col].abs() > max_val {
            max_val = aug[row][col].abs();
            max_row = row;
        }
    }
    max_row
}

/// Eliminate column `col` from every row below it using the pivot row.
fn eliminate_below_pivot(aug: &mut [Vec<f64>], col: usize, n: usize, pivot: f64) {
    for row in (col + 1)..n {
        let factor = aug[row][col] / pivot;
        for j in col..=n {
            aug[row][j] -= factor * aug[col][j];
        }
    }
}

/// Back substitution over the row-echelon augmented matrix.
fn back_substitute(aug: &[Vec<f64>], n: usize) -> Vec<f64> {
    let mut result = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in (i + 1)..n {
            sum -= aug[i][j] * result[j];
        }
        if aug[i][i].abs() > 1e-15 {
            result[i] = sum / aug[i][i];
        }
    }
    result
}

/// Power iteration to find the dominant eigenvector of a symmetric matrix.
fn power_iteration(matrix: &[Vec<f64>], max_iter: usize) -> (f64, Vec<f64>) {
    let n = matrix.len();
    let mut v: Vec<f64> = vec![1.0 / (n as f64).sqrt(); n];

    let mut eigenvalue = 0.0;
    for _ in 0..max_iter {
        // w = A * v
        let w: Vec<f64> = (0..n)
            .map(|i| {
                matrix[i]
                    .iter()
                    .zip(v.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
            })
            .collect();

        // Eigenvalue estimate = v^T * w
        eigenvalue = v.iter().zip(w.iter()).map(|(a, b)| a * b).sum::<f64>();

        // Normalize
        let norm: f64 = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-15 {
            break;
        }
        v = w.iter().map(|x| x / norm).collect();
    }

    (eigenvalue, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_regression() {
        // y = 2*x + 1
        let x = vec![vec![1.0], vec![2.0], vec![3.0], vec![4.0], vec![5.0]];
        let y = vec![3.0, 5.0, 7.0, 9.0, 11.0];
        let result = linear_regression(&x, &y);
        assert!((result.coefficients[0] - 2.0).abs() < 0.01);
        assert!((result.intercept - 1.0).abs() < 0.01);
        assert!(result.r_squared > 0.99);
    }

    // spec: MEDULLA-CLUSTER-R008.4
    #[test]
    fn test_kmeans() {
        let data = vec![
            vec![1.0, 1.0],
            vec![1.1, 1.1],
            vec![0.9, 0.9],
            vec![5.0, 5.0],
            vec![5.1, 5.1],
            vec![4.9, 4.9],
        ];
        let result = kmeans(&data, 2, 100).unwrap();
        assert_eq!(result.labels.len(), 6);
        // First three should be same cluster, last three another
        assert_eq!(result.labels[0], result.labels[1]);
        assert_eq!(result.labels[3], result.labels[4]);
        assert_ne!(result.labels[0], result.labels[3]);
    }

    #[test]
    fn test_pca() {
        let data = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
            vec![5.0, 6.0],
            vec![7.0, 8.0],
        ];
        let result = pca(&data, 1);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.transformed.len(), 4);
    }

    #[test]
    fn test_dataset_stats() {
        let data = vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.0]];
        let stats = compute_stats(&data);
        assert_eq!(stats.n_samples, 3);
        assert_eq!(stats.n_features, 2);
        assert!((stats.means[0] - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_train_test_split() {
        let data = vec![vec![1.0], vec![2.0], vec![3.0], vec![4.0], vec![5.0]];
        let labels = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        // Deterministic (no shuffle) so the size assertions are unambiguous.
        let (x_train, x_test, y_train, y_test) = train_test_split(&data, &labels, 0.2, false, 0);
        assert_eq!(x_train.len(), 4);
        assert_eq!(x_test.len(), 1);
        assert_eq!(y_train.len(), 4);
        assert_eq!(y_test.len(), 1);

        // Shuffled split preserves sizes and partitions all rows.
        let (xs_tr, xs_te, _, _) = train_test_split(&data, &labels, 0.2, true, 42);
        assert_eq!(xs_tr.len(), 4);
        assert_eq!(xs_te.len(), 1);
    }

    /// A linear system `(A, b)` and its exact solution `x`.
    type LinearSystemCase = (Vec<Vec<f64>>, Vec<f64>, Vec<f64>);

    #[test]
    fn solve_linear_system_pivots_and_skips_singular_columns() {
        let cases: [LinearSystemCase; 2] = [
            // A zero leading pivot: only a row swap can solve it.
            (
                vec![vec![0.0, 1.0], vec![1.0, 0.0]],
                vec![2.0, 3.0],
                vec![3.0, 2.0],
            ),
            // Rows 0 and 1 are identical, so column 1's pivot is zero after
            // eliminating column 0. That column must be skipped (no division
            // by zero into row 2) and its unknown left at 0.
            (
                vec![
                    vec![1.0, 1.0, 0.0],
                    vec![1.0, 1.0, 0.0],
                    vec![0.0, 0.0, 1.0],
                ],
                vec![2.0, 2.0, 3.0],
                vec![2.0, 0.0, 3.0],
            ),
        ];
        for (a, b, expected) in cases {
            let x = solve_linear_system(&a, &b);
            assert_eq!(x, expected, "A={a:?} b={b:?}");
        }
    }
}
