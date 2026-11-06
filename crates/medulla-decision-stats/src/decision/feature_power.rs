//! Feature-power metrics of a `DecisionEval` receipt (MEDULLA-DECISION-ENGINE-R117.1.2).
//!
//! For each named feature the admitted items are put in time order and reduced
//! to two series: the feature (its mean over the item's candidates) and the
//! outcome (the share of candidates acceptable for a gold item; the executed
//! option's success for a logged one, a censored run reading as missing). The
//! report is the rank information coefficient of the two, its decay over the
//! next `max_lag` items, the information ratio at the effective sample size,
//! and a bootstrap interval seeded by the dataset digest, so the same dataset
//! always gives the same report.

use medulla_decision_types::decision::statistical::dataset::{
    ItemLabel, LabelledDataset, LabelledItem,
};
use medulla_decision_types::decision::{FeaturePowerReport, IcInterval, QuantScaleTag};
use medulla_numeric::evaluation::skill::{
    bootstrap_ic_interval, ic_decay, information_coefficient_checked, information_ratio,
};
use medulla_numeric::series::window::{pearson, ranks};
use medulla_types::solve_scalar::digest_text;

use super::quant::{item_rows, q32, raw_value};
use super::refusal::{bounded, Refusal, RefusalResult};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

/// Domain of the dataset digest that seeds the bootstrap.
pub const FEATURE_POWER_SEED_DOMAIN: &str = "eg/decision-feature-power-seed/v1";
/// Fewest ordered items a feature-power report needs.
pub const MIN_FEATURE_POWER_ITEMS: usize = 4;

/// The knobs of one feature-power computation: `eval.ic_max_lag`,
/// `eval.bootstrap_resamples` and `eval.ci_level`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeaturePowerKnobs {
    pub max_lag: usize,
    pub resamples: usize,
    pub level: f64,
}

/// The outcome of one item on a unit scale, or `NaN` for a censored run.
fn outcome_of(item: &LabelledItem) -> f64 {
    match &item.label {
        ItemLabel::Gold { acceptable, .. } => {
            let good = acceptable
                .iter()
                .filter(|id| item.index_of(id).is_some())
                .count();
            good as f64 / item.candidate_ids.len().max(1) as f64
        }
        ItemLabel::Logged(logged) => match logged.evaluation.success {
            Some(true) => 1.0,
            Some(false) => 0.0,
            None => f64::NAN,
        },
    }
}

/// The seed of the bootstrap: the head of the dataset digest.
fn seed_of(dataset: &LabelledDataset) -> u64 {
    let digest = digest_text(FEATURE_POWER_SEED_DOMAIN, dataset);
    let hex = digest.rsplit(':').next().unwrap_or_default();
    u64::from_str_radix(hex.get(..16).unwrap_or("0"), 16).unwrap_or(0)
}

/// Lag-1 autocorrelation based effective sample size, inside `[1, n]`.
fn effective_sample_size(feature: &[f64]) -> f64 {
    let n = feature.len() as f64;
    let rho = pearson(&feature[..feature.len() - 1], &feature[1..]).unwrap_or(0.0);
    (n * (1.0 - rho) / (1.0 + rho)).clamp(1.0, n)
}

/// Feature power of every feature of `dataset` over `items`; empty when fewer
/// than [`MIN_FEATURE_POWER_ITEMS`] items carry an outcome.
pub fn feature_power(
    dataset: &LabelledDataset,
    items: &[&LabelledItem],
    knobs: &FeaturePowerKnobs,
) -> RefusalResult<Vec<FeaturePowerReport>> {
    let mut ordered: Vec<&LabelledItem> = items.to_vec();
    ordered.sort_by(|a, b| (a.recorded_at_ms, &a.item_id).cmp(&(b.recorded_at_ms, &b.item_id)));
    // A censored run has no outcome: it leaves the series, the rest keep their order.
    ordered.retain(|item| !outcome_of(item).is_nan());
    let outcomes: Vec<f64> = ordered.iter().map(|item| outcome_of(item)).collect();
    if ordered.len() < MIN_FEATURE_POWER_ITEMS {
        return Ok(Vec::new());
    }
    let rows = ordered
        .iter()
        .map(|item| item_rows(dataset, item))
        .collect::<RefusalResult<Vec<_>>>()?;
    let seed = seed_of(dataset);
    let lags = knobs.max_lag.min(ordered.len() - 2).max(1);
    let numeric = |e: medulla_numeric::NumericError| {
        Refusal::new(StatisticalErrorCode::NumericRefused, e.to_string())
    };
    let mut reports = Vec::new();
    for (column, name) in dataset.feature_names.iter().enumerate() {
        let feature: Vec<f64> = rows
            .iter()
            .map(|item_rows| {
                let sum: f64 = item_rows
                    .iter()
                    .map(|row| raw_value(row[column], QuantScaleTag::Q32))
                    .sum();
                sum / item_rows.len().max(1) as f64
            })
            .collect();
        let rank_ic = rank_ic_of(&feature, &outcomes);
        let decay = ic_decay(&feature, &outcomes, lags).map_err(numeric)?;
        let ess = effective_sample_size(&feature);
        let (lower, upper) =
            bootstrap_ic_interval(&feature, &outcomes, knobs.resamples, knobs.level, seed)
                .map_err(numeric)?;
        reports.push(FeaturePowerReport {
            feature: name.clone(),
            rank_ic: q32(rank_ic)?,
            ic_decay: bounded(
                decay
                    .into_iter()
                    .map(q32)
                    .collect::<RefusalResult<Vec<_>>>()?,
            )?,
            information_ratio: q32(information_ratio(rank_ic, ess))?,
            effective_sample_size: q32(ess)?,
            ic_interval: IcInterval {
                level: q32(knobs.level)?,
                lower: q32(lower)?,
                upper: q32(upper)?,
            },
        });
    }
    Ok(reports)
}

/// The same-observation rank IC; `0` when a side is constant.
fn rank_ic_of(feature: &[f64], outcomes: &[f64]) -> f64 {
    information_coefficient_checked(&ranks(feature), &ranks(outcomes)).unwrap_or(0.0)
}
