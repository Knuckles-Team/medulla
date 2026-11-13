//! The per-horizon categorical Brier gate (MEDULLA-FINANCE-PRIMITIVES-R002.11.2) and
//! the policy-bound sizing arithmetic (R002.11.3) of the served regime forecast.
//!
//! Pure functions over wire types: no graph read, no clock, no write. A refusal is
//! a `(code, detail)` pair whose code is a registered `ServerErrorCode`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_numeric::calibration::metrics::brier_score;
use medulla_numeric::calibration::ProbabilityMatrix;
use medulla_types::regime_forecast::{
    HeldoutSample, HorizonCalibration, HorizonSizing, RegimeCalibrationInput,
    RegimeForecastRequest, RegimeSizingPolicy, SizingAbstainReason, CALIBRATION_FUTURE_LEAK,
    CALIBRATION_POLICY_INVALID, CALIBRATION_ROW_DUPLICATE, CALIBRATION_ROW_MISMATCH,
    CALIBRATION_ROW_MISSING, CALIBRATION_SAMPLES_LOW,
};
use sha2::{Digest, Sha256};

use super::finance_accounting::datetime_ns;

pub(super) type Refusal = (&'static str, String);

fn refuse<T>(code: &'static str, detail: impl Into<String>) -> Result<T, Refusal> {
    Err((code, detail.into()))
}

pub(super) fn digest_of<T: serde::Serialize>(value: &T) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("wire values serialize"),
    ))
}

/// The model a forecast was drawn from, as the gate needs it.
pub(super) struct GateModel<'a> {
    pub(super) request: &'a RegimeForecastRequest,
    pub(super) state_ids: &'a [String],
    pub(super) as_of_ns: i64,
    /// The top-state probability of each requested horizon, in horizon order.
    pub(super) top_state_probability: &'a [f64],
    pub(super) max_brier_ceiling: f64,
}

fn check_policy(input: &RegimeCalibrationInput, model: &GateModel<'_>) -> Result<(), Refusal> {
    let policy = &input.policy;
    if policy.policy_id.is_empty() || policy.revision.is_empty() {
        return refuse(
            CALIBRATION_POLICY_INVALID,
            "the policy needs an id and a revision",
        );
    }
    let mut seen = BTreeSet::new();
    for thresholds in &policy.horizons {
        if !seen.insert(thresholds.horizon) {
            return refuse(
                CALIBRATION_POLICY_INVALID,
                format!("horizon {} appears twice", thresholds.horizon),
            );
        }
        let brier_ok = thresholds.max_brier.is_finite()
            && (0.0..=model.max_brier_ceiling).contains(&thresholds.max_brier);
        let top_ok = thresholds.min_top_state_probability.is_finite()
            && (0.0..=1.0).contains(&thresholds.min_top_state_probability);
        if !brier_ok || !top_ok || thresholds.minimum_heldout_examples == 0 {
            return refuse(
                CALIBRATION_POLICY_INVALID,
                format!(
                    "horizon {} has a threshold out of range",
                    thresholds.horizon
                ),
            );
        }
    }
    let requested: BTreeSet<u32> = model.request.horizons.iter().copied().collect();
    if seen != requested {
        return refuse(
            CALIBRATION_POLICY_INVALID,
            "the policy must name every requested horizon exactly once and no other",
        );
    }
    Ok(())
}

struct Row<'a> {
    sample: &'a HeldoutSample,
    origin_ns: i64,
    label: usize,
}

fn check_rows<'a>(
    input: &'a RegimeCalibrationInput,
    model: &GateModel<'_>,
) -> Result<Vec<Row<'a>>, Refusal> {
    let request = model.request;
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut rows = Vec::with_capacity(input.samples.len());
    for sample in &input.samples {
        if !ids.insert(sample.sample_id.as_str()) {
            return refuse(
                CALIBRATION_ROW_DUPLICATE,
                format!("sample id {} repeats", sample.sample_id),
            );
        }
        let (Some(origin_ns), Some(target_ns)) = (
            datetime_ns(&sample.origin_as_of),
            datetime_ns(&sample.target_observation_at),
        ) else {
            return refuse(
                CALIBRATION_ROW_MISMATCH,
                format!("sample {} has an unreadable instant", sample.sample_id),
            );
        };
        if !keys.insert((sample.source_series_id.as_str(), origin_ns, sample.horizon)) {
            return refuse(
                CALIBRATION_ROW_DUPLICATE,
                format!(
                    "sample {} repeats a (source, origin, horizon) row",
                    sample.sample_id
                ),
            );
        }
        if origin_ns >= target_ns || target_ns > model.as_of_ns {
            return refuse(
                CALIBRATION_FUTURE_LEAK,
                format!(
                    "sample {} must have origin before its target and a target not after the forecast as-of",
                    sample.sample_id
                ),
            );
        }
        let label = model
            .state_ids
            .iter()
            .position(|id| *id == sample.realized_state_id);
        let matches_model = sample.model_revision == request.model_revision
            && sample.source_series_id == request.source_series_id
            && sample.ordered_state_ids == model.state_ids
            && sample.predicted_probability_vector.len() == model.state_ids.len()
            && request.horizons.contains(&sample.horizon)
            && !sample.scoring_revision.is_empty();
        let Some(label) = label.filter(|_| matches_model) else {
            return refuse(
                CALIBRATION_ROW_MISMATCH,
                format!(
                    "sample {} does not match the model revision, source, state order or horizons",
                    sample.sample_id
                ),
            );
        };
        rows.push(Row {
            sample,
            origin_ns,
            label,
        });
    }
    let origins: BTreeSet<i64> = rows.iter().map(|row| row.origin_ns).collect();
    for origin in origins {
        for horizon in &request.horizons {
            if !keys.iter().any(|(_, o, h)| *o == origin && h == horizon) {
                return refuse(
                    CALIBRATION_ROW_MISSING,
                    format!("an origin has no row for horizon {horizon}"),
                );
            }
        }
    }
    Ok(rows)
}

/// Score every requested horizon against its held-out rows and the policy.
pub(super) fn calibrate(
    input: &RegimeCalibrationInput,
    model: &GateModel<'_>,
) -> Result<Vec<HorizonCalibration>, Refusal> {
    check_policy(input, model)?;
    let rows = check_rows(input, model)?;
    let policy_digest = digest_of(&input.policy);
    let state_ids_digest = digest_of(&model.state_ids);
    let mut out = Vec::with_capacity(model.request.horizons.len());
    for (index, horizon) in model.request.horizons.iter().enumerate() {
        let thresholds = input
            .policy
            .horizons
            .iter()
            .find(|t| t.horizon == *horizon)
            .expect("check_policy matched every horizon");
        let mut group: Vec<&Row<'_>> = rows
            .iter()
            .filter(|r| r.sample.horizon == *horizon)
            .collect();
        group.sort_by(|a, b| {
            (a.origin_ns, &a.sample.sample_id).cmp(&(b.origin_ns, &b.sample.sample_id))
        });
        let count = group.len() as u64;
        if count < thresholds.minimum_heldout_examples {
            return refuse(
                CALIBRATION_SAMPLES_LOW,
                format!(
                    "horizon {horizon} has {count} held-out rows, the policy needs {}",
                    thresholds.minimum_heldout_examples
                ),
            );
        }
        let scoring_revision = group[0].sample.scoring_revision.clone();
        if group
            .iter()
            .any(|r| r.sample.scoring_revision != scoring_revision)
        {
            return refuse(
                CALIBRATION_ROW_MISMATCH,
                format!("horizon {horizon} mixes scoring revisions"),
            );
        }
        let vectors: Vec<Vec<f64>> = group
            .iter()
            .map(|r| r.sample.predicted_probability_vector.clone())
            .collect();
        let labels: Vec<usize> = group.iter().map(|r| r.label).collect();
        let brier = ProbabilityMatrix::from_rows(&vectors)
            .and_then(|matrix| brier_score(&matrix, &labels))
            .map_err(|error| {
                (
                    CALIBRATION_ROW_MISMATCH,
                    format!("horizon {horizon} rows are not scorable: {error}"),
                )
            })?;
        let samples: Vec<&HeldoutSample> = group.iter().map(|r| r.sample).collect();
        out.push(HorizonCalibration {
            horizon: *horizon,
            dataset_digest: digest_of(&samples),
            model_revision: model.request.model_revision.clone(),
            state_ids_digest: state_ids_digest.clone(),
            scoring_revision,
            calibration_policy_digest: policy_digest.clone(),
            sample_count: count,
            brier,
            applied_thresholds: thresholds.clone(),
            passed: brier <= thresholds.max_brier
                && model.top_state_probability[index] >= thresholds.min_top_state_probability,
        });
    }
    Ok(out)
}

/// A sizing abstention: its reason and a human detail.
pub(super) type Abstain = (SizingAbstainReason, String);

/// `expected_target_weight_h = sum_s p_hs * w_s`, capped by refusal: a figure past
/// the risk cap abstains and no value is returned.
pub(super) fn size(
    policy: &RegimeSizingPolicy,
    state_ids: &[String],
    horizons: &[u32],
    vectors: &[Vec<f64>],
) -> Result<Vec<HorizonSizing>, Abstain> {
    let weights: BTreeMap<&str, i64> = policy
        .target_weight_by_state
        .iter()
        .map(|(state, weight)| (state.as_str(), *weight))
        .collect();
    let ordered: Result<Vec<i64>, Abstain> = state_ids
        .iter()
        .map(|id| {
            weights.get(id.as_str()).copied().ok_or_else(|| {
                (
                    SizingAbstainReason::PolicyWeightMissing,
                    format!("the policy has no weight for state {id}"),
                )
            })
        })
        .collect();
    let ordered = ordered?;
    let mut out = Vec::with_capacity(horizons.len());
    for (horizon, vector) in horizons.iter().zip(vectors) {
        let expected: f64 = vector
            .iter()
            .zip(&ordered)
            .map(|(p, w)| p * *w as f64)
            .sum();
        let ticks = expected.round() as i64;
        if ticks.unsigned_abs() > policy.risk_cap.unsigned_abs() {
            return Err((
                SizingAbstainReason::RiskCapExceeded,
                format!(
                    "horizon {horizon} expected weight {ticks} exceeds the risk cap {}",
                    policy.risk_cap
                ),
            ));
        }
        out.push(HorizonSizing {
            horizon: *horizon,
            expected_target_weight: ticks,
        });
    }
    Ok(out)
}
