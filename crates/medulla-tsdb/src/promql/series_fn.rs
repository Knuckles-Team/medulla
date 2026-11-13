//! PromQL series functions (R117.1.1): `medulla_<func>(range-vector…, params…)` for every
//! function in `medulla_types::series_expr::SIGNATURES`, named exactly as the SQL window
//! functions are (`medulla_lag`, `medulla_rmean`, `medulla_rcorr`, ...).
//!
//! The planner compiles each call to the shared `Kernel` series program
//! (`crate::derive::Program`) — the one engine UQL `DERIVE` and the SQL window functions
//! run — and holds no evaluator of its own. Each series' points inside the range window
//! are fed to a fresh program in timestamp order and the value at the last point is the
//! sample (no sample while the kernel is warming up or where it is undefined). A
//! two-series function pairs series whose labels agree (ignoring the metric name) and
//! feeds the timestamps both carry.

use medulla_types::series_expr::{SeriesExpr, Signature, SIGNATURES};

use super::{
    as_range, err, Evaluator, Expr, InstantSample, Labels, PromqlError, RangeSeries, Value,
    METRIC_NAME,
};
use crate::derive::Program;
use crate::point::Ts;

/// The prefix every series function carries in PromQL and SQL.
const PREFIX: &str = "medulla_";

/// The signature a PromQL function name selects.
pub(super) fn named(name: &str) -> Option<&'static Signature> {
    let bare = name.strip_prefix(PREFIX)?;
    SIGNATURES.iter().find(|s| s.name == bare)
}

/// The channel names the program reads, one per series argument.
const CHANNELS: [&str; 2] = ["a", "b"];

fn without_name(labels: &Labels) -> Labels {
    let mut l = labels.clone();
    l.remove(METRIC_NAME);
    l
}

/// Run `sig` over the series arguments and return the sample at each series' last point.
fn evaluate(
    sig: &Signature,
    args: &[Vec<RangeSeries>],
    params: &[f64],
) -> Result<Vec<InstantSample>, PromqlError> {
    let call = SeriesExpr::Call {
        func: sig.func,
        args: CHANNELS[..sig.series]
            .iter()
            .map(|c| SeriesExpr::Channel {
                name: (*c).to_string(),
            })
            .collect(),
        params: params.to_vec(),
    };
    let template =
        Program::compile(&call).map_err(|e| PromqlError(format!("{}{}: {e}", PREFIX, sig.name)))?;
    let mut out = Vec::new();
    for left in &args[0] {
        let key = without_name(&left.labels);
        let joined: Vec<(Ts, f64, f64)> = if sig.series == 1 {
            left.points.iter().map(|&(t, v)| (t, v, 0.0)).collect()
        } else {
            let Some(right) = args[1].iter().find(|r| without_name(&r.labels) == key) else {
                continue;
            };
            left.points
                .iter()
                .filter_map(|&(t, v)| {
                    right
                        .points
                        .iter()
                        .find(|(rt, _)| *rt == t)
                        .map(|&(_, w)| (t, v, w))
                })
                .collect()
        };
        let mut program = template.clone();
        let mut last = None;
        for (_, a, b) in joined {
            last = program.step(&|channel| match channel {
                "a" => Some(a),
                "b" => Some(b),
                _ => None,
            });
        }
        if let Some(value) = last {
            out.push(InstantSample { labels: key, value });
        }
    }
    Ok(out)
}

impl Evaluator<'_> {
    /// `medulla_<func>(range-vector…, params…)` at instant `t`.
    pub(super) fn eval_series_call(
        &self,
        sig: &Signature,
        args: &[Expr],
        t: Ts,
    ) -> Result<Value, PromqlError> {
        let want = sig.series + sig.params;
        if args.len() != want {
            return err(format!(
                "{}{} expects {} range vector(s) then {} number(s)",
                PREFIX, sig.name, sig.series, sig.params
            ));
        }
        let mut series = Vec::with_capacity(sig.series);
        for arg in &args[..sig.series] {
            series.push(as_range(self.eval_instant(arg, t)?, sig.name)?);
        }
        let mut params = Vec::with_capacity(sig.params);
        for arg in &args[sig.series..] {
            params.push(self.scalar_arg(arg, t, sig.name)?);
        }
        Ok(Value::Instant(evaluate(sig, &series, &params)?))
    }
}
