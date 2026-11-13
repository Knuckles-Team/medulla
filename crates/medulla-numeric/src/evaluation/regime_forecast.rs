//! Multi-horizon Markov regime forecast (MEDULLA-FINANCE-PRIMITIVES-R002.11.1).
//!
//! `p_h = p0 * T^h` for each requested horizon `h`, with `p0` the posterior state
//! probability vector and `T` the row-stochastic transition matrix of a fitted
//! regime HMM (the shared kernel of [`super::regime_hmm`]). The state order is the
//! order of `state_ids` and is kept in every output. This is the one owner of the
//! multi-step transition powers; callers never supply `p0` or `T` themselves.

/// Why a forecast input was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegimeForecastRefusal {
    /// The horizons are empty, not strictly increasing, not positive or too far.
    HorizonInvalid(String),
    /// The matrix, the posterior or the state ids are not a valid Markov model.
    MatrixInvalid(String),
}

impl RegimeForecastRefusal {
    /// The stable wire code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::HorizonInvalid(_) => "FORECAST_HORIZON_INVALID",
            Self::MatrixInvalid(_) => "FORECAST_MATRIX_INVALID",
        }
    }

    /// The human detail.
    pub fn detail(&self) -> &str {
        match self {
            Self::HorizonInvalid(detail) | Self::MatrixInvalid(detail) => detail,
        }
    }
}

fn matrix_invalid<T>(detail: impl Into<String>) -> Result<T, RegimeForecastRefusal> {
    Err(RegimeForecastRefusal::MatrixInvalid(detail.into()))
}

fn check_probability_vector(
    what: &str,
    row: &[f64],
    tolerance: f64,
) -> Result<(), RegimeForecastRefusal> {
    if row.iter().any(|p| !p.is_finite()) {
        return matrix_invalid(format!("{what} holds a non-finite value"));
    }
    if row.iter().any(|p| *p < 0.0) {
        return matrix_invalid(format!("{what} holds a negative probability"));
    }
    if (row.iter().sum::<f64>() - 1.0).abs() > tolerance {
        return matrix_invalid(format!("{what} does not sum to 1 within {tolerance:e}"));
    }
    Ok(())
}

/// Validate a model: unique non-empty state ids, a probability vector `p0` and a
/// square row-stochastic matrix, all of the same order.
pub fn validate_model(
    state_ids: &[String],
    p0: &[f64],
    matrix: &[Vec<f64>],
    tolerance: f64,
) -> Result<(), RegimeForecastRefusal> {
    let n = state_ids.len();
    if n == 0 {
        return matrix_invalid("the model has no states");
    }
    let mut seen = std::collections::BTreeSet::new();
    if state_ids.iter().any(|id| id.is_empty() || !seen.insert(id)) {
        return matrix_invalid("state ids must be non-empty and unique");
    }
    if p0.len() != n || matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        return matrix_invalid("p0 and the matrix must match the state count");
    }
    check_probability_vector("p0", p0, tolerance)?;
    for (i, row) in matrix.iter().enumerate() {
        check_probability_vector(&format!("matrix row {i}"), row, tolerance)?;
    }
    Ok(())
}

/// The horizons must be non-empty, strictly increasing, positive and at most `max`.
pub fn validate_horizons(horizons: &[u32], max: u32) -> Result<(), RegimeForecastRefusal> {
    if horizons.is_empty() {
        return Err(RegimeForecastRefusal::HorizonInvalid(
            "horizons are empty".into(),
        ));
    }
    if horizons[0] == 0 || horizons.windows(2).any(|w| w[0] >= w[1]) {
        return Err(RegimeForecastRefusal::HorizonInvalid(
            "horizons must be positive and strictly increasing".into(),
        ));
    }
    if horizons[horizons.len() - 1] > max {
        return Err(RegimeForecastRefusal::HorizonInvalid(format!(
            "the largest horizon exceeds {max}"
        )));
    }
    Ok(())
}

/// `p0 * T^h` for every horizon, in the state order of the inputs. One vector per
/// horizon; each sums to 1 within the model tolerance.
pub fn forecast(
    state_ids: &[String],
    p0: &[f64],
    matrix: &[Vec<f64>],
    horizons: &[u32],
    max_horizon: u32,
    tolerance: f64,
) -> Result<Vec<Vec<f64>>, RegimeForecastRefusal> {
    validate_horizons(horizons, max_horizon)?;
    validate_model(state_ids, p0, matrix, tolerance)?;
    let n = p0.len();
    let mut current = p0.to_vec();
    let mut step = 0_u32;
    let mut out = Vec::with_capacity(horizons.len());
    for &horizon in horizons {
        while step < horizon {
            let mut next = vec![0.0; n];
            for (i, p) in current.iter().enumerate() {
                for (j, t) in matrix[i].iter().enumerate() {
                    next[j] += p * t;
                }
            }
            current = next;
            step += 1;
        }
        out.push(current.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        ["bull", "flat", "bear"].map(String::from).to_vec()
    }

    fn matrix() -> Vec<Vec<f64>> {
        vec![
            vec![0.8, 0.15, 0.05],
            vec![0.2, 0.6, 0.2],
            vec![0.1, 0.3, 0.6],
        ]
    }

    #[test]
    fn one_step_is_the_vector_matrix_product() {
        let out = forecast(&ids(), &[1.0, 0.0, 0.0], &matrix(), &[1], 100, 1e-9).unwrap();
        assert_eq!(out, vec![vec![0.8, 0.15, 0.05]]);
    }

    #[test]
    fn horizons_must_be_positive_increasing_and_bounded() {
        for horizons in [vec![], vec![0], vec![2, 2], vec![3, 1], vec![101]] {
            let refusal =
                forecast(&ids(), &[1.0, 0.0, 0.0], &matrix(), &horizons, 100, 1e-9).unwrap_err();
            assert_eq!(refusal.code(), "FORECAST_HORIZON_INVALID", "{horizons:?}");
        }
    }

    #[test]
    fn a_malformed_model_is_refused() {
        let mut negative = matrix();
        negative[0] = vec![1.1, -0.1, 0.0];
        let mut loose = matrix();
        loose[1][0] = 0.3;
        let mut nonfinite = matrix();
        nonfinite[2][2] = f64::NAN;
        for bad in [negative, loose, nonfinite, vec![vec![1.0]]] {
            let refusal = forecast(&ids(), &[1.0, 0.0, 0.0], &bad, &[1], 100, 1e-9).unwrap_err();
            assert_eq!(refusal.code(), "FORECAST_MATRIX_INVALID");
        }
        let duplicate = ["a", "a", "b"].map(String::from).to_vec();
        let refusal = forecast(&duplicate, &[1.0, 0.0, 0.0], &matrix(), &[1], 100, 1e-9);
        assert_eq!(refusal.unwrap_err().code(), "FORECAST_MATRIX_INVALID");
        let refusal = forecast(&ids(), &[0.5, 0.2, 0.2], &matrix(), &[1], 100, 1e-9);
        assert_eq!(refusal.unwrap_err().code(), "FORECAST_MATRIX_INVALID");
    }
}
