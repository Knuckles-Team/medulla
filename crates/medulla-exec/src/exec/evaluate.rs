//! `EVALUATE` (MEDULLA-FINANCE-PRIMITIVES-R002.2.3.15): one numeric evaluation kernel
//! run over the named value channels of each series, through the one registry
//! [`medulla_numeric::evaluation::registry::evaluate`].
//!
//! The grammar, the wire op and the kernel table
//! ([`medulla_query_types::series_expr::EVALUATE_KERNELS`]) are built in the query
//! layer; this module is the executor, modelled on [`super::skill`]. Per series it takes
//! the rows, in timestamp order, where every named channel has a value; the input rows
//! are replaced by report rows (a series row id, so a later `DERIVE` can read them)
//! whose score is the kernel's headline statistic and whose value channels are its
//! output fields. A kernel refusal fails the plan as `"<KERNEL_CODE>: <detail>"`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_numeric::evaluation::registry;
use medulla_query_types::series_expr::{evaluate_kernel, EvaluateKernel, EvaluateOp};
use medulla_types::numeric_kernel::{
    KernelCode, KernelRefusal, NumericEvaluateOp, NumericKernelOutput,
};

use super::derive::{named_series_groups, read_channel};
use crate::rowset::{parse_series_row_id, series_row_id, RowSet, ValueChannels};

/// The largest integer an `f64` holds exactly; a `usize` parameter above it is refused.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// One series' aligned points: the timestamp and the value of each named channel.
type Aligned = Vec<(i64, Vec<f64>)>;

/// The text of a kernel refusal, `"<KERNEL_CODE>: <detail>"`.
fn refusal_text(refusal: &KernelRefusal) -> String {
    format!("{}: {}", refusal.code.as_str(), refusal.detail)
}

fn kernel_error(code: KernelCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// A kernel together with its resolved parameters (defaults applied, required keys
/// present), so a parameter refusal never depends on the data.
enum Resolved {
    DieboldMariano {
        h: usize,
    },
    DeflatedSharpe {
        trials: usize,
        observed: Option<f64>,
    },
    PurgedCpcv {
        samples: usize,
        groups: usize,
        test: usize,
        purge: usize,
        embargo: usize,
    },
    ProbabilityOfBacktestOverfit,
    Brier,
    OuCalibrate {
        dt: f64,
    },
    Adf {
        max_lag: usize,
    },
    RegimeHmm {
        states: usize,
        iter: usize,
        tol: f64,
    },
}

/// The parameters of one stage, read by key (ASCII case-insensitive).
struct Params<'a> {
    kernel: &'static str,
    op: &'a EvaluateOp,
}

impl Params<'_> {
    fn raw(&self, key: &str) -> Option<f64> {
        self.op
            .params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| *v)
    }

    fn required(&self, key: &str) -> Result<f64, String> {
        self.raw(key).ok_or_else(|| {
            kernel_error(
                KernelCode::ParamInvalid,
                format!("{key} is required by {}", self.kernel),
            )
        })
    }

    fn count_of(&self, key: &str, value: f64) -> Result<usize, String> {
        if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= MAX_EXACT_INTEGER {
            return Ok(value as usize);
        }
        Err(kernel_error(
            KernelCode::ParamInvalid,
            format!("{key} is not a non-negative integer"),
        ))
    }

    fn count(&self, key: &str, default: usize) -> Result<usize, String> {
        match self.raw(key) {
            Some(value) => self.count_of(key, value),
            None => Ok(default),
        }
    }

    fn required_count(&self, key: &str) -> Result<usize, String> {
        let value = self.required(key)?;
        self.count_of(key, value)
    }
}

/// Resolve the stage's kernel and parameters, refusing an unknown kernel, a wrong
/// channel arity, an unknown parameter key and a missing required key.
fn resolve(op: &EvaluateOp) -> Result<(&'static EvaluateKernel, Resolved), String> {
    let kernel = evaluate_kernel(&op.kernel).ok_or_else(|| {
        format!(
            "UQL_E_EVALUATE_KERNEL: `{}` is not an EVALUATE kernel",
            op.kernel
        )
    })?;
    if op.channels.len() != kernel.channels {
        return Err(format!(
            "UQL_E_EVALUATE_KERNEL: {} reads {} channel(s), got {}",
            kernel.name,
            kernel.channels,
            op.channels.len()
        ));
    }
    if let Some((key, _)) = op
        .params
        .iter()
        .find(|(key, _)| !kernel.params.iter().any(|p| p.eq_ignore_ascii_case(key)))
    {
        return Err(kernel_error(
            KernelCode::ParamInvalid,
            format!("{key} is not a parameter of {}", kernel.name),
        ));
    }
    let params = Params {
        kernel: kernel.name,
        op,
    };
    let resolved = match kernel.name {
        "diebold_mariano" => Resolved::DieboldMariano {
            h: params.count("HORIZON", 1)?,
        },
        "deflated_sharpe" => Resolved::DeflatedSharpe {
            trials: params.required_count("TRIALS")?,
            observed: params.raw("OBSERVED"),
        },
        "purged_cpcv" => Resolved::PurgedCpcv {
            samples: params.required_count("SAMPLES")?,
            groups: params.required_count("GROUPS")?,
            test: params.required_count("TEST")?,
            purge: params.required_count("PURGE")?,
            embargo: params.required_count("EMBARGO")?,
        },
        "probability_of_backtest_overfit" => Resolved::ProbabilityOfBacktestOverfit,
        "brier" => Resolved::Brier,
        "ou_calibrate" => Resolved::OuCalibrate {
            dt: params.raw("DT").unwrap_or(1.0),
        },
        "adf" => Resolved::Adf {
            max_lag: params.count("MAXLAG", 0)?,
        },
        "regime_hmm" => Resolved::RegimeHmm {
            states: params.required_count("STATES")?,
            iter: params.count("ITER", 100)?,
            tol: params.raw("TOL").unwrap_or(1e-6),
        },
        other => {
            return Err(format!(
                "UQL_E_EVALUATE_KERNEL: `{other}` has no executor in this build"
            ))
        }
    };
    Ok((kernel, resolved))
}

/// Sample Sharpe of `returns`: the mean over the sample standard deviation (n - 1).
fn sample_sharpe(returns: &[f64]) -> Result<f64, String> {
    if returns.len() < 2 {
        return Err(kernel_error(
            KernelCode::InputShort,
            "the sample Sharpe needs at least two points",
        ));
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Ok(mean / variance.sqrt())
}

impl Resolved {
    /// The registry op for one series' channel columns (one per named channel).
    fn op(&self, columns: &[Vec<f64>]) -> Result<NumericEvaluateOp, String> {
        Ok(match self {
            Self::DieboldMariano { h } => NumericEvaluateOp::DieboldMariano {
                losses_a: columns[0].clone(),
                losses_b: columns[1].clone(),
                h: *h,
            },
            Self::DeflatedSharpe { trials, observed } => NumericEvaluateOp::DeflatedSharpe {
                observed_sr: match observed {
                    Some(observed) => *observed,
                    None => sample_sharpe(&columns[0])?,
                },
                n_trials: *trials,
                sr_returns: columns[0].clone(),
            },
            Self::Brier => NumericEvaluateOp::Brier {
                forecasts: columns[0].clone(),
                outcomes: columns[1].clone(),
            },
            Self::OuCalibrate { dt } => NumericEvaluateOp::OuCalibrate {
                spread: columns[0].clone(),
                dt: *dt,
            },
            Self::Adf { max_lag } => NumericEvaluateOp::Adf {
                series: columns[0].clone(),
                max_lag: *max_lag,
            },
            Self::RegimeHmm { states, iter, tol } => NumericEvaluateOp::RegimeHmm {
                observations: columns[0].clone(),
                n_states: *states,
                max_iter: *iter,
                tol: *tol,
            },
            Self::PurgedCpcv { .. } | Self::ProbabilityOfBacktestOverfit => {
                return Err("UQL_E_EVALUATE_KERNEL: not a per-series kernel".to_string())
            }
        })
    }
}

fn fields(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn flag(value: bool) -> f64 {
    if value {
        1.0
    } else {
        0.0
    }
}

/// A report row's headline score and its value-channel fields.
fn report(output: &NumericKernelOutput) -> Result<(f64, Vec<(String, f64)>), String> {
    Ok(match output {
        NumericKernelOutput::Scalar { value } => (*value, fields(&[("value", *value)])),
        NumericKernelOutput::DieboldMariano(dm) => (
            dm.statistic,
            fields(&[
                ("statistic", dm.statistic),
                ("p_value", dm.p_value),
                ("a_better", flag(dm.a_better)),
            ]),
        ),
        NumericKernelOutput::OuParams(ou) => (
            ou.theta,
            fields(&[
                ("theta", ou.theta),
                ("mu", ou.mu),
                ("sigma", ou.sigma),
                ("half_life", ou.half_life),
                ("sigma_eq", ou.sigma_eq),
            ]),
        ),
        NumericKernelOutput::Adf(adf) => (
            adf.statistic,
            fields(&[
                ("statistic", adf.statistic),
                ("used_lag", adf.used_lag as f64),
                ("n_obs", adf.n_obs as f64),
                ("crit_1pct", adf.crit_1pct),
                ("crit_5pct", adf.crit_5pct),
                ("crit_10pct", adf.crit_10pct),
                ("p_value_approx", adf.p_value_approx),
                ("stationary_1pct", flag(adf.stationary_1pct)),
                ("stationary_5pct", flag(adf.stationary_5pct)),
                ("stationary_10pct", flag(adf.stationary_10pct)),
            ]),
        ),
        NumericKernelOutput::Regime(regime) => {
            let mut out = fields(&[
                ("log_likelihood", regime.log_likelihood),
                ("n_iterations", regime.n_iterations as f64),
            ]);
            for (j, (mean, std)) in regime.means.iter().zip(&regime.stds).enumerate() {
                out.push((format!("mean_{j}"), *mean));
                out.push((format!("std_{j}"), *std));
            }
            (regime.log_likelihood, out)
        }
        _ => return Err("UQL_E_EVALUATE_KERNEL: the kernel returned an unexpected output".into()),
    })
}

/// Collect one report row into the output.
fn push_row(
    id: String,
    score: f64,
    row: Vec<(String, f64)>,
    scored: &mut Vec<(String, f32)>,
    channels: &mut ValueChannels,
) {
    scored.push((id.clone(), score as f32));
    channels.insert(id, row.into_iter().collect());
}

/// The series' rows in timestamp order where every named channel has a value.
fn aligned(
    rows: &[crate::rowset::Row],
    indices: &[usize],
    values: &ValueChannels,
    channels: &[String],
) -> Aligned {
    indices
        .iter()
        .filter_map(|&i| {
            let row = &rows[i];
            let ts = parse_series_row_id(&row.id)?.1;
            let point = channels
                .iter()
                .map(|channel| read_channel(values, row, channel))
                .collect::<Option<Vec<f64>>>()?;
            Some((ts, point))
        })
        .collect()
}

/// `purged_cpcv`: no channel and no series; one row per split.
fn purged_cpcv(resolved: &Resolved) -> Result<RowSet, String> {
    let Resolved::PurgedCpcv {
        samples,
        groups,
        test,
        purge,
        embargo,
    } = resolved
    else {
        return Err("UQL_E_EVALUATE_KERNEL: not purged_cpcv".to_string());
    };
    let output = registry::evaluate(&NumericEvaluateOp::PurgedCpcv {
        n_samples: *samples,
        n_groups: *groups,
        n_test_groups: *test,
        purge_window: *purge,
        embargo: *embargo,
    })
    .map_err(|refusal| refusal_text(&refusal))?;
    let NumericKernelOutput::Splits { splits } = output else {
        return Err("UQL_E_EVALUATE_KERNEL: the kernel returned an unexpected output".into());
    };
    let mut scored = Vec::new();
    let mut channels = ValueChannels::new();
    for (i, split) in splits.iter().enumerate() {
        let first = split.test.iter().min().copied().unwrap_or(0);
        let last = split.test.iter().max().copied().unwrap_or(0);
        let n_test = split.test.len() as f64;
        push_row(
            series_row_id("purged_cpcv:eval", i as i64),
            n_test,
            fields(&[
                ("split", i as f64),
                ("n_train", split.train.len() as f64),
                ("n_test", n_test),
                ("test_first", first as f64),
                ("test_last", last as f64),
            ]),
            &mut scored,
            &mut channels,
        );
    }
    Ok(RowSet::from_scored(scored).with_values(channels))
}

/// `probability_of_backtest_overfit`: each series is one strategy and each timestamp where
/// every series has both channels is one split.
fn backtest_overfit(per_series: &BTreeMap<&str, Aligned>) -> Result<RowSet, String> {
    if per_series.len() < 2 {
        return Err(kernel_error(
            KernelCode::InputShort,
            "probability_of_backtest_overfit needs at least two series",
        ));
    }
    let by_ts: Vec<BTreeMap<i64, &Vec<f64>>> = per_series
        .values()
        .map(|points| points.iter().map(|(ts, point)| (*ts, point)).collect())
        .collect();
    let mut common: BTreeSet<i64> = by_ts[0].keys().copied().collect();
    for series in &by_ts[1..] {
        common.retain(|ts| series.contains_key(ts));
    }
    let insample = common
        .iter()
        .map(|ts| by_ts.iter().map(|series| series[ts][0]).collect())
        .collect();
    let oos = common
        .iter()
        .map(|ts| by_ts.iter().map(|series| series[ts][1]).collect())
        .collect();
    let output =
        registry::evaluate(&NumericEvaluateOp::ProbabilityOfBacktestOverfit { insample, oos })
            .map_err(|refusal| refusal_text(&refusal))?;
    let (score, row) = report(&output)?;
    let mut scored = Vec::new();
    let mut channels = ValueChannels::new();
    push_row(
        series_row_id("eval:probability_of_backtest_overfit", 0),
        score,
        row,
        &mut scored,
        &mut channels,
    );
    Ok(RowSet::from_scored(scored).with_values(channels))
}

/// Replace the input's series rows with the report rows of one `EVALUATE` kernel.
pub(crate) fn evaluate_op(input: RowSet, op: &EvaluateOp, max_work: u64) -> Result<RowSet, String> {
    let (kernel, resolved) = resolve(op)?;
    if matches!(resolved, Resolved::PurgedCpcv { .. }) {
        return purged_cpcv(&resolved);
    }
    let (rows, values) = input.into_parts();
    let per_series: BTreeMap<&str, Aligned> = named_series_groups(&rows)
        .into_iter()
        .map(|(series, indices)| (series, aligned(&rows, &indices, &values, &op.channels)))
        .collect();
    let points: u64 = per_series.values().map(|points| points.len() as u64).sum();
    if points > max_work {
        return Err(kernel_error(
            KernelCode::WorkBudget,
            format!("{points} aligned points exceed {max_work}"),
        ));
    }
    if matches!(resolved, Resolved::ProbabilityOfBacktestOverfit) {
        return backtest_overfit(&per_series);
    }
    let mut scored = Vec::new();
    let mut channels = ValueChannels::new();
    for (series, points) in &per_series {
        let columns: Vec<Vec<f64>> = (0..kernel.channels)
            .map(|c| points.iter().map(|(_, point)| point[c]).collect())
            .collect();
        let output = registry::evaluate(&resolved.op(&columns)?)
            .map_err(|refusal| refusal_text(&refusal))?;
        let (score, row) = report(&output)?;
        let id = series_row_id(&format!("{series}:eval@{}", kernel.name), 0);
        push_row(id, score, row, &mut scored, &mut channels);
    }
    Ok(RowSet::from_scored(scored).with_values(channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kernel: &str, channels: &[&str], params: &[(&str, f64)]) -> EvaluateOp {
        EvaluateOp {
            kernel: kernel.to_string(),
            channels: channels.iter().map(|c| c.to_string()).collect(),
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    /// One series `s` of `n` points carrying the channels `a` and `b`.
    fn series(n: i64) -> RowSet {
        let mut values = ValueChannels::new();
        let mut ids = Vec::new();
        for t in 1..=n {
            let id = series_row_id("s", t);
            let a = ((t * 7) % 11) as f64 / 10.0;
            let b = ((t * 5) % 13) as f64 / 10.0;
            values.insert(
                id.clone(),
                [("a".to_string(), a), ("b".to_string(), b)]
                    .into_iter()
                    .collect(),
            );
            ids.push(id);
        }
        RowSet::from_ids(ids).with_values(values)
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.15
    #[test]
    fn refusals_and_defaults() {
        let unknown = evaluate_op(series(12), &op("no_such", &["a"], &[]), 1_000).unwrap_err();
        assert!(unknown.starts_with("UQL_E_EVALUATE_KERNEL"), "{unknown}");

        let missing =
            evaluate_op(series(12), &op("deflated_sharpe", &["a"], &[]), 1_000).unwrap_err();
        assert_eq!(
            missing,
            "KERNEL_PARAM_INVALID: TRIALS is required by deflated_sharpe"
        );

        let budget =
            evaluate_op(series(12), &op("diebold_mariano", &["a", "b"], &[]), 5).unwrap_err();
        assert_eq!(budget, "KERNEL_WORK_BUDGET: 12 aligned points exceed 5");

        let omitted =
            evaluate_op(series(12), &op("diebold_mariano", &["a", "b"], &[]), 1_000).unwrap();
        let explicit = evaluate_op(
            series(12),
            &op("diebold_mariano", &["a", "b"], &[("HORIZON", 1.0)]),
            1_000,
        )
        .unwrap();
        assert_eq!(omitted.ids(), vec!["s:eval@diebold_mariano@0"]);
        assert_eq!(omitted.ids(), explicit.ids());
        for channel in ["statistic", "p_value", "a_better"] {
            assert_eq!(
                omitted.value("s:eval@diebold_mariano@0", channel),
                explicit.value("s:eval@diebold_mariano@0", channel)
            );
            assert!(omitted.value("s:eval@diebold_mariano@0", channel).is_some());
        }
    }
}
