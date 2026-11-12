//! The numeric kernel registry (MEDULLA-FINANCE-PRIMITIVES-R002.2.5): the one place a
//! [`NumericEvaluateOp`] is mapped to the shared kernel that answers it.
//!
//! Each arm validates the request against fixed ceilings and the kernel's domain,
//! then calls the one kernel that already holds the maths: no arm computes
//! anything itself. Results are the kernel's exact `f64` values, never rounded. A
//! refusal is a [`KernelRefusal`] carrying one stable code.

use medulla_types::numeric_kernel::{
    CvSplit, DieboldMariano, EwmaSmoothing, KernelCode, KernelRefusal, NumericEvaluateOp,
    NumericKernelOutput, MAX_CPCV_SPLITS, MAX_KERNEL_INPUT,
};

use super::{backtest, regime_hmm, skill, statarb};
use crate::calibration::metrics::binary_brier_score;
use crate::error::NumericError;
use crate::series::{apply, Rolling, Smoothing, Spec};

type Refused<T> = Result<T, KernelRefusal>;

fn refuse<T>(code: KernelCode, detail: impl Into<String>) -> Refused<T> {
    Err(KernelRefusal::new(code, detail))
}

/// Map a kernel's own typed error to the stable refusal code.
fn from_kernel(error: NumericError) -> KernelRefusal {
    let code = match &error {
        NumericError::Shape(_) => KernelCode::LengthMismatch,
        NumericError::LinAlg(_) => KernelCode::Singular,
        NumericError::Resource(_) => KernelCode::StateBudget,
        NumericError::Bounds(_) | NumericError::Type(_) | NumericError::Random(_) => {
            KernelCode::ParamInvalid
        }
    };
    KernelRefusal::new(code, error.to_string())
}

fn within_budget(what: &str, len: usize) -> Refused<()> {
    if len > MAX_KERNEL_INPUT {
        return refuse(
            KernelCode::StateBudget,
            format!("`{what}` holds {len} values; the ceiling is {MAX_KERNEL_INPUT}"),
        );
    }
    Ok(())
}

fn finite(what: &str, values: &[f64]) -> Refused<()> {
    if values.iter().any(|v| !v.is_finite()) {
        return refuse(
            KernelCode::InputNonfinite,
            format!("`{what}` holds a non-finite value"),
        );
    }
    Ok(())
}

fn finite_scalar(what: &str, value: f64) -> Refused<()> {
    finite(what, &[value])
}

fn at_least(what: &str, len: usize, least: usize) -> Refused<()> {
    if len < least {
        return refuse(
            KernelCode::InputShort,
            format!("`{what}` holds {len} values; at least {least} are needed"),
        );
    }
    Ok(())
}

fn same_len(a: &str, x: usize, b: &str, y: usize) -> Refused<()> {
    if x != y {
        return refuse(
            KernelCode::LengthMismatch,
            format!("`{a}` holds {x} values and `{b}` holds {y}"),
        );
    }
    Ok(())
}

fn non_negative(what: &str, value: f64) -> Refused<()> {
    finite_scalar(what, value)?;
    if value < 0.0 {
        return refuse(KernelCode::ParamInvalid, format!("`{what}` is negative"));
    }
    Ok(())
}

fn positive(what: &str, value: f64) -> Refused<()> {
    finite_scalar(what, value)?;
    if value <= 0.0 {
        return refuse(
            KernelCode::ParamInvalid,
            format!("`{what}` is not above zero"),
        );
    }
    Ok(())
}

fn constant(values: &[f64]) -> bool {
    values.windows(2).all(|w| w[0] == w[1])
}

fn matrix(what: &str, rows: &[Vec<f64>]) -> Refused<usize> {
    let width = rows.first().map_or(0, Vec::len);
    let total = rows.iter().map(Vec::len).sum::<usize>();
    within_budget(what, total.max(rows.len()))?;
    if rows.iter().any(|row| row.len() != width) {
        return refuse(
            KernelCode::LengthMismatch,
            format!("the rows of `{what}` differ in length"),
        );
    }
    for row in rows {
        finite(what, row)?;
    }
    Ok(width)
}

/// `C(n, k)` capped just above [`MAX_CPCV_SPLITS`], so the check cannot overflow.
fn splits_needed(n: usize, k: usize) -> u128 {
    let cap = MAX_CPCV_SPLITS as u128 + 1;
    let k = k.min(n - k.min(n));
    let mut acc: u128 = 1;
    for i in 0..k as u128 {
        acc = acc * (n as u128 - i) / (i + 1);
        if acc >= cap {
            return cap;
        }
    }
    acc
}

fn scalar(value: f64) -> NumericKernelOutput {
    NumericKernelOutput::Scalar { value }
}

/// Evaluate one op through its shared kernel.
pub fn evaluate(op: &NumericEvaluateOp) -> Result<NumericKernelOutput, KernelRefusal> {
    match op {
        NumericEvaluateOp::RollingZScore { values, window } => {
            within_budget("values", values.len())?;
            at_least("values", values.len(), 1)?;
            finite("values", values)?;
            let out =
                apply(Spec::Rolling(Rolling::Zscore, *window), values).map_err(from_kernel)?;
            Ok(NumericKernelOutput::Series { values: out })
        }
        NumericEvaluateOp::Ewma { values, smoothing } => {
            within_budget("values", values.len())?;
            at_least("values", values.len(), 1)?;
            finite("values", values)?;
            let smoothing = match *smoothing {
                EwmaSmoothing::Span(s) => Smoothing::Span(s),
                EwmaSmoothing::HalfLife(h) => Smoothing::HalfLife(h),
            };
            let out = apply(Spec::Ewma(smoothing), values).map_err(from_kernel)?;
            Ok(NumericKernelOutput::Series { values: out })
        }
        NumericEvaluateOp::InformationCoefficient {
            signal,
            forward_returns,
        } => {
            within_budget("signal", signal.len())?;
            within_budget("forward_returns", forward_returns.len())?;
            same_len(
                "signal",
                signal.len(),
                "forward_returns",
                forward_returns.len(),
            )?;
            at_least("signal", signal.len(), 2)?;
            finite("signal", signal)?;
            finite("forward_returns", forward_returns)?;
            if constant(signal) || constant(forward_returns) {
                return refuse(KernelCode::Singular, "a constant side has no correlation");
            }
            skill::information_coefficient_checked(signal, forward_returns)
                .map(scalar)
                .map_err(from_kernel)
        }
        NumericEvaluateOp::InformationRatio { ic, n_independent } => {
            finite_scalar("ic", *ic)?;
            finite_scalar("n_independent", *n_independent)?;
            skill::information_ratio_checked(*ic, *n_independent)
                .map(scalar)
                .map_err(from_kernel)
        }
        NumericEvaluateOp::EffectiveIndependentN { returns_matrix } => {
            let width = matrix("returns_matrix", returns_matrix)?;
            at_least("returns_matrix", returns_matrix.len(), 1)?;
            at_least("returns_matrix row", width, 2)?;
            skill::effective_independent_n_checked(returns_matrix)
                .map(scalar)
                .map_err(from_kernel)
        }
        NumericEvaluateOp::DieboldMariano {
            losses_a,
            losses_b,
            h,
        } => {
            within_budget("losses_a", losses_a.len())?;
            within_budget("losses_b", losses_b.len())?;
            same_len("losses_a", losses_a.len(), "losses_b", losses_b.len())?;
            at_least("losses_a", losses_a.len(), 10)?;
            finite("losses_a", losses_a)?;
            finite("losses_b", losses_b)?;
            if *h == 0 {
                return refuse(KernelCode::ParamInvalid, "`h` is at least one");
            }
            let dm =
                backtest::diebold_mariano_checked(losses_a, losses_b, *h).map_err(from_kernel)?;
            Ok(NumericKernelOutput::DieboldMariano(DieboldMariano {
                statistic: dm.statistic,
                p_value: dm.p_value,
                a_better: dm.a_better,
            }))
        }
        NumericEvaluateOp::DeflatedSharpe {
            observed_sr,
            n_trials,
            sr_returns,
        } => {
            within_budget("sr_returns", sr_returns.len())?;
            finite_scalar("observed_sr", *observed_sr)?;
            finite("sr_returns", sr_returns)?;
            at_least("sr_returns", sr_returns.len(), 4)?;
            if *n_trials < 1 {
                return refuse(KernelCode::ParamInvalid, "`n_trials` is at least one");
            }
            backtest::deflated_sharpe_ratio_checked(*observed_sr, *n_trials, sr_returns)
                .map(scalar)
                .map_err(from_kernel)
        }
        NumericEvaluateOp::PurgedCpcv {
            n_samples,
            n_groups,
            n_test_groups,
            purge_window,
            embargo,
        } => {
            within_budget("n_samples", *n_samples)?;
            if *n_groups == 0 || *n_test_groups == 0 || n_test_groups > n_groups {
                return refuse(
                    KernelCode::ParamInvalid,
                    "cross-validation needs 0 < n_test_groups <= n_groups",
                );
            }
            if n_samples < n_groups {
                return refuse(
                    KernelCode::InputShort,
                    "cross-validation needs at least one sample per group",
                );
            }
            if splits_needed(*n_groups, *n_test_groups) > MAX_CPCV_SPLITS as u128 {
                return refuse(
                    KernelCode::StateBudget,
                    format!("more than {MAX_CPCV_SPLITS} splits requested"),
                );
            }
            let splits = backtest::purged_cpcv_splits_checked(
                *n_samples,
                *n_groups,
                *n_test_groups,
                *purge_window,
                *embargo,
            )
            .map_err(from_kernel)?;
            Ok(NumericKernelOutput::Splits {
                splits: splits
                    .into_iter()
                    .map(|s| CvSplit {
                        train: s.train,
                        test: s.test,
                    })
                    .collect(),
            })
        }
        NumericEvaluateOp::ProbabilityOfBacktestOverfit { insample, oos } => {
            same_len("insample", insample.len(), "oos", oos.len())?;
            at_least("insample", insample.len(), 1)?;
            let width = matrix("insample", insample)?;
            let oos_width = matrix("oos", oos)?;
            same_len("insample row", width, "oos row", oos_width)?;
            at_least("insample row", width, 2)?;
            backtest::probability_of_backtest_overfit_checked(insample, oos)
                .map(scalar)
                .map_err(from_kernel)
        }
        NumericEvaluateOp::Brier {
            forecasts,
            outcomes,
        } => {
            within_budget("forecasts", forecasts.len())?;
            within_budget("outcomes", outcomes.len())?;
            same_len("forecasts", forecasts.len(), "outcomes", outcomes.len())?;
            at_least("forecasts", forecasts.len(), 1)?;
            finite("forecasts", forecasts)?;
            finite("outcomes", outcomes)?;
            binary_brier_score(forecasts, outcomes)
                .map(scalar)
                .map_err(|e| KernelRefusal::new(KernelCode::ParamInvalid, e.to_string()))
        }
        NumericEvaluateOp::KalmanLevel {
            observations,
            f,
            q,
            h,
            r,
            x0,
            p0,
        } => {
            within_budget("observations", observations.len())?;
            at_least("observations", observations.len(), 1)?;
            finite("observations", observations)?;
            for (name, v) in [("f", f), ("h", h), ("x0", x0)] {
                finite_scalar(name, *v)?;
            }
            for (name, v) in [("q", q), ("r", r), ("p0", p0)] {
                non_negative(name, *v)?;
            }
            Ok(NumericKernelOutput::Kalman(statarb::kalman_filter_1d(
                observations,
                *f,
                *q,
                *h,
                *r,
                *x0,
                *p0,
            )))
        }
        NumericEvaluateOp::KalmanBeta {
            market_returns,
            asset_returns,
            q,
            r,
            beta0,
            p0,
        } => {
            within_budget("market_returns", market_returns.len())?;
            within_budget("asset_returns", asset_returns.len())?;
            same_len(
                "market_returns",
                market_returns.len(),
                "asset_returns",
                asset_returns.len(),
            )?;
            at_least("market_returns", market_returns.len(), 1)?;
            finite("market_returns", market_returns)?;
            finite("asset_returns", asset_returns)?;
            finite_scalar("beta0", *beta0)?;
            for (name, v) in [("q", q), ("r", r), ("p0", p0)] {
                non_negative(name, *v)?;
            }
            Ok(NumericKernelOutput::Kalman(statarb::kalman_beta(
                market_returns,
                asset_returns,
                *q,
                *r,
                *beta0,
                *p0,
            )))
        }
        NumericEvaluateOp::KalmanVolatility {
            returns,
            q,
            r,
            log_var0,
            p0,
            annualization,
        } => {
            within_budget("returns", returns.len())?;
            at_least("returns", returns.len(), 1)?;
            finite("returns", returns)?;
            if let Some(v) = log_var0 {
                finite_scalar("log_var0", *v)?;
            }
            for (name, v) in [("q", q), ("r", r), ("p0", p0)] {
                non_negative(name, *v)?;
            }
            positive("annualization", *annualization)?;
            Ok(NumericKernelOutput::Vector {
                values: statarb::kalman_volatility(returns, *q, *r, *log_var0, *p0, *annualization),
            })
        }
        NumericEvaluateOp::OuCalibrate { spread, dt } => {
            within_budget("spread", spread.len())?;
            at_least("spread", spread.len(), 3)?;
            finite("spread", spread)?;
            positive("dt", *dt)?;
            if constant(spread) {
                return refuse(
                    KernelCode::Singular,
                    "a constant spread cannot be regressed",
                );
            }
            Ok(NumericKernelOutput::OuParams(statarb::ou_calibrate(
                spread, *dt,
            )))
        }
        NumericEvaluateOp::OuThresholds {
            theta,
            mu,
            sigma,
            sigma_eq,
            cost,
        } => {
            finite_scalar("mu", *mu)?;
            positive("theta", *theta)?;
            positive("sigma", *sigma)?;
            positive("sigma_eq", *sigma_eq)?;
            non_negative("cost", *cost)?;
            let params = medulla_types::numeric_kernel::OuParams {
                theta: *theta,
                mu: *mu,
                sigma: *sigma,
                half_life: std::f64::consts::LN_2 / *theta,
                sigma_eq: *sigma_eq,
            };
            Ok(NumericKernelOutput::OuThresholds(
                statarb::ou_optimal_thresholds(&params, *cost),
            ))
        }
        NumericEvaluateOp::Adf { series, max_lag } => {
            within_budget("series", series.len())?;
            finite("series", series)?;
            if *max_lag > series.len() {
                return refuse(
                    KernelCode::ParamInvalid,
                    "`max_lag` exceeds the series length",
                );
            }
            at_least("series", series.len(), max_lag + 4)?;
            let result = statarb::adf_test(series, *max_lag);
            if result.n_obs == 0 {
                return refuse(KernelCode::Singular, "the ADF regression is singular");
            }
            Ok(NumericKernelOutput::Adf(result))
        }
        NumericEvaluateOp::RegimeHmm {
            observations,
            n_states,
            max_iter,
            tol,
        } => {
            within_budget("observations", observations.len())?;
            at_least("observations", observations.len(), 2)?;
            finite("observations", observations)?;
            positive("tol", *tol)?;
            if *n_states == 0 || *n_states > 64 || *max_iter == 0 || *max_iter > 10_000 {
                return refuse(
                    KernelCode::ParamInvalid,
                    "`n_states` is 1..=64 and `max_iter` is 1..=10000",
                );
            }
            let work = (observations.len() as u128)
                * (*n_states as u128)
                * (*n_states as u128)
                * (*max_iter as u128);
            if work > 2_000_000_000 {
                return refuse(KernelCode::StateBudget, "the fit exceeds the work budget");
            }
            Ok(NumericKernelOutput::Regime(regime_hmm::detect_regimes(
                observations,
                *n_states,
                *max_iter,
                *tol,
            )))
        }
    }
}
