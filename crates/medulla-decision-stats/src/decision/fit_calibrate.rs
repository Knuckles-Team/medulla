//! Calibrating a full-label listwise head on its held-out items (EH-024,
//! EH-005).
//!
//! Three steps, each on the held-out items only: temperature scaling (one
//! inverse temperature minimising the held-out listwise loss), an
//! acceptability-set split-conformal threshold at the policy's alpha -- the
//! weighted variant when any item carries an audit inclusion weight -- and a
//! Learn-then-Test act threshold certifying `P(wrong | act) <= epsilon` with
//! probability at least `1 - delta`. A step whose sample is too small makes
//! no claim: no act threshold is certified, and the head never acts.
//!
//! The conformal and act thresholds are computed on the probabilities the
//! head SERVES: a linear head's float softmax, or -- for the resident scorer
//! -- its fixed-point softmax over the scored options (EH-293). One core,
//! [`calibrate_logits`], serves both.

use medulla_decision_types::decision::statistical::{CalibrationMethod, StatisticalErrorCode};
use medulla_decision_types::{
    decision::statistical::dataset::LabelledDataset, decision::statistical::dataset::LabelledItem,
};
use medulla_decision_types::{
    decision::statistical::head::DecisionHeadBody, decision::statistical::head::HeadCalibration,
    decision::statistical::head::IsotonicBlock, decision::statistical::head::IsotonicTable,
};
use medulla_decision_types::{
    decision::statistical::RiskMethod, decision::statistical::RiskStatement,
};
use medulla_decision_types::{decision::QuantisedValue, decision::StatisticalPolicy};
use medulla_types::contract::BoundedVec;

use super::fit::Example;
use super::head_eval::{apply_isotonic, logit, scaled_softmax, top_index};
use super::quant::{item_rows, level, q32, unit_wire, value_of};
use super::refusal::{Refusal, RefusalResult};
use super::scorer::fixed::{to_f64, to_q32};
use super::scorer::forward::{read, served_probabilities, EXCLUDED_LOGIT};
use super::scorer::legal::LegalSet;
use super::targets::{audit_weight, targets};
use crate::{
    conformal::acceptability_score, conformal::realised_coverage_interval,
    conformal::split_conformal, conformal::weighted_split_conformal, conformal::Threshold,
};
use medulla_numeric::calibration::isotonic::fit_isotonic_weighted;
use medulla_numeric::{
    detkernel::optimise::minimise_convex_bounded, detkernel::optimise::DEFAULT_BISECTION_STEPS,
};
use medulla_numeric::{risk::calibrate_selective_risk, risk::RiskTarget, risk::SampleGate};

/// Inverse-temperature search bracket.
pub const INVERSE_TEMPERATURE_RANGE: (f64, f64) = (1e-3, 1e3);
/// `2^32` as a working value.
const Q32_ONE: f64 = 4_294_967_296.0;
/// Number of act thresholds on the fixed grid `0.995, 0.985, ..., 0.505`.
pub const ACT_GRID_POINTS: u32 = 50;

fn logits_of(examples: &[Example], weights: &[f64]) -> Vec<Vec<f64>> {
    examples
        .iter()
        .map(|e| e.rows.iter().map(|row| logit(weights, row)).collect())
        .collect()
}

/// How a head turns logits into the probabilities it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reader {
    /// Float softmax over every option (the linear heads).
    Linear,
    /// Fixed-point softmax over the scored options (the resident scorer).
    Fixed,
}

/// The logit of an option the scorer did not score, as a working value.
fn excluded(z: f64) -> bool {
    z <= to_f64(EXCLUDED_LOGIT)
}

fn served(reader: Reader, logits: &[f64], beta: QuantisedValue) -> RefusalResult<Vec<f64>> {
    match reader {
        Reader::Linear => scaled_softmax(logits, value_of(beta)),
        Reader::Fixed => {
            let fixed: Vec<i64> = logits.iter().map(|&z| fixed_logit(z)).collect();
            Ok(served_probabilities(&fixed, to_q32(beta))
                .into_iter()
                .map(to_f64)
                .collect())
        }
    }
}

/// A working logit back on Q32. Scorer logits are Q32 values read exactly,
/// so this is exact.
fn fixed_logit(z: f64) -> i64 {
    if excluded(z) {
        EXCLUDED_LOGIT
    } else {
        (z * Q32_ONE) as i64
    }
}

/// `d loss / d beta`; options the scorer did not score carry no gradient.
fn temperature_slope(examples: &[Example], logits: &[Vec<f64>], beta: f64) -> f64 {
    let mut total = 0.0;
    for (example, z) in examples.iter().zip(logits) {
        let Ok(p) = scaled_softmax(z, beta) else {
            continue;
        };
        let mut inner = 0.0;
        for ((pj, tj), zj) in p.iter().zip(&example.targets).zip(z) {
            if !excluded(*zj) {
                inner += (pj - tj) * zj;
            }
        }
        total += example.weight * inner;
    }
    total
}

fn acceptable(example: &Example) -> Vec<usize> {
    (0..example.targets.len())
        .filter(|&i| example.targets[i] > 0.0)
        .collect()
}

fn set_threshold(threshold: Threshold) -> f64 {
    match threshold {
        Threshold::Empty => -1.0,
        Threshold::Finite(cut) => cut,
        Threshold::Infinite => 1.0,
    }
}

fn act_grid() -> Vec<f64> {
    (0..ACT_GRID_POINTS)
        .map(|i| 0.995 - 0.01 * f64::from(i))
        .collect()
}

struct Held {
    probabilities: Vec<Vec<f64>>,
    scores: Vec<f64>,
    weights: Vec<f64>,
    top: Vec<f64>,
    wrong: Vec<bool>,
}

fn held_out(
    examples: &[Example],
    logits: &[Vec<f64>],
    reader: Reader,
    beta: QuantisedValue,
    isotonic: Option<&IsotonicTable>,
) -> RefusalResult<Held> {
    let mut held = Held {
        probabilities: Vec::new(),
        scores: Vec::new(),
        weights: Vec::new(),
        top: Vec::new(),
        wrong: Vec::new(),
    };
    for (example, z) in examples.iter().zip(logits) {
        let p = served(reader, z, beta)?;
        let p = match isotonic {
            Some(table) => apply_isotonic(table, &p)?,
            None => p,
        };
        let nonconformity: Vec<f64> = p.iter().map(|pj| 1.0 - pj).collect();
        let good = acceptable(example);
        held.scores
            .push(acceptability_score(&nonconformity, &good)?);
        let top = top_index(&p).unwrap_or(0);
        held.top.push(p[top]);
        held.wrong.push(!good.contains(&top));
        held.weights.push(example.weight);
        held.probabilities.push(p);
    }
    Ok(held)
}

fn conformal_threshold(held: &Held, policy: &StatisticalPolicy) -> RefusalResult<f64> {
    let alpha = level(policy.alpha)?;
    let weighted = held.weights.iter().any(|w| *w != 1.0);
    let quantile = if weighted {
        weighted_split_conformal(&held.scores, &held.weights, 1.0, alpha)?
    } else {
        split_conformal(&held.scores, alpha)?
    };
    Ok(set_threshold(quantile.threshold()))
}

fn certified_act(
    held: &Held,
    policy: &StatisticalPolicy,
    n: u64,
) -> RefusalResult<Option<(f64, RiskStatement)>> {
    let target = RiskTarget {
        epsilon: level(policy.epsilon)?,
        delta: level(policy.delta)?,
        gate: SampleGate::new(policy.n_min.max(1))?,
    };
    let certificate = calibrate_selective_risk(&held.top, &held.wrong, &act_grid(), target)?;
    Ok(certificate.certified.map(|c| {
        (
            c.threshold,
            RiskStatement {
                method: RiskMethod::LearnThenTest,
                epsilon: policy.epsilon,
                delta: policy.delta,
                n_calibration: n,
            },
        )
    }))
}

/// Calibrate a fitted linear listwise head on its held-out examples.
/// `None` when there are no held-out examples to calibrate on.
///
/// `method` is the fit request's calibration method (MEDULLA-DECISION-ENGINE-R032.2.1):
/// `None` and `Conformal`/`Temperature` keep the ladder's choice, `Isotonic`
/// fits a monotone map over the positive-class score of a two-class head
/// first and runs the split-conformal threshold over the mapped
/// probabilities; any other method refuses with `CALIBRATION_METHOD_UNSUPPORTED`.
pub(crate) fn calibrate(
    examples: &[Example],
    weights: &[f64],
    policy: &StatisticalPolicy,
    synthetic: bool,
    method: Option<CalibrationMethod>,
) -> RefusalResult<Option<HeadCalibration>> {
    let isotonic = checked_method(method)?;
    let logits = logits_of(examples, weights);
    calibrate_logits(
        examples,
        &logits,
        Reader::Linear,
        policy,
        synthetic,
        isotonic,
    )
}

/// Whether the requested method is isotonic; refuses any method the fit does
/// not serve.
pub(crate) fn checked_method(method: Option<CalibrationMethod>) -> RefusalResult<bool> {
    match method {
        None | Some(CalibrationMethod::Conformal) | Some(CalibrationMethod::Temperature) => {
            Ok(false)
        }
        Some(CalibrationMethod::Isotonic) => Ok(true),
        Some(other) => Err(Refusal::new(
            StatisticalErrorCode::CalibrationMethodUnsupported,
            format!("the fit does not serve the {other:?} calibration method"),
        )),
    }
}

/// The isotonic map from the positive-class score (the first option's
/// temperature-scaled probability) to its observed target share, copied block
/// by block in increasing order.
fn isotonic_table(
    examples: &[Example],
    logits: &[Vec<f64>],
    reader: Reader,
    beta: QuantisedValue,
) -> RefusalResult<IsotonicTable> {
    if examples.iter().any(|example| example.targets.len() != 2) {
        return Err(Refusal::new(
            StatisticalErrorCode::IsotonicRequiresBinary,
            "isotonic calibration applies to two-option items only",
        ));
    }
    let mut scores = Vec::with_capacity(examples.len());
    let mut targets = Vec::with_capacity(examples.len());
    let mut weights = Vec::with_capacity(examples.len());
    for (example, z) in examples.iter().zip(logits) {
        scores.push(value_of(q32(served(reader, z, beta)?[0])?));
        targets.push(example.targets[0]);
        weights.push(example.weight);
    }
    let fit = fit_isotonic_weighted(&scores, &targets, &weights)?;
    let blocks = fit
        .blocks()
        .into_iter()
        .map(|(lower, upper, value)| {
            Ok(IsotonicBlock {
                lower: q32(lower)?,
                upper: q32(upper)?,
                value: q32(value)?,
            })
        })
        .collect::<RefusalResult<Vec<_>>>()?;
    let blocks = BoundedVec::new(blocks)
        .map_err(|detail| Refusal::new(StatisticalErrorCode::HeadInvalid, detail))?;
    Ok(IsotonicTable { blocks })
}

/// Calibrate the resident scorer on its own served outputs over the held-out
/// items: each item's targets and weight, and its fixed-point logits.
pub(crate) fn calibrate_scorer(
    head: &DecisionHeadBody,
    dataset: &LabelledDataset,
    held_out: &[&LabelledItem],
    policy: &StatisticalPolicy,
) -> RefusalResult<Option<HeadCalibration>> {
    let mut examples = Vec::new();
    let mut logits = Vec::new();
    for item in held_out {
        let Some((targets, weight)) = targets(item) else {
            continue;
        };
        let rows = item_rows(dataset, item)?;
        let views: Vec<&[i64]> = rows.iter().map(Vec::as_slice).collect();
        let Ok(scoring) = read(head, &views, &LegalSet::derive(views.len(), &[]))? else {
            continue;
        };
        logits.push(scoring.logits.into_iter().map(to_f64).collect());
        examples.push(Example {
            rows: Vec::new(),
            targets,
            weight: weight * audit_weight(item),
        });
    }
    calibrate_logits(
        &examples,
        &logits,
        Reader::Fixed,
        policy,
        dataset.synthetic,
        false,
    )
}

/// The calibration core over logits served through `reader`. `None` when
/// there are no held-out examples to calibrate on.
pub(crate) fn calibrate_logits(
    examples: &[Example],
    logits: &[Vec<f64>],
    reader: Reader,
    policy: &StatisticalPolicy,
    synthetic: bool,
    isotonic: bool,
) -> RefusalResult<Option<HeadCalibration>> {
    if examples.is_empty() {
        return Ok(None);
    }
    let (low, high) = INVERSE_TEMPERATURE_RANGE;
    let beta = minimise_convex_bounded(
        |b| temperature_slope(examples, logits, b),
        low,
        high,
        DEFAULT_BISECTION_STEPS,
    )?;
    let beta_q = q32(beta)?;
    let table = if isotonic {
        Some(isotonic_table(examples, logits, reader, beta_q)?)
    } else {
        None
    };
    let held = held_out(examples, logits, reader, beta_q, table.as_ref())?;
    let n = examples.len() as u64;
    let threshold = conformal_threshold(&held, policy)?;
    let (lower, upper) = realised_coverage_interval(level(policy.alpha)?, n, level(policy.delta)?)?
        .unwrap_or((0.0, 1.0));
    let act = certified_act(&held, policy, n)?;
    Ok(Some(HeadCalibration {
        inverse_temperature: beta_q,
        alpha: policy.alpha,
        set_threshold: q32(threshold)?,
        coverage_lower: unit_wire(lower)?,
        coverage_upper: unit_wire(upper)?,
        act_threshold: act.as_ref().map(|(t, _)| q32(*t)).transpose()?,
        risk: act.map(|(_, risk)| risk),
        n_calibration: n,
        synthetic,
        isotonic: table,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::head_eval::isotonic_value;
    use medulla_decision_types::decision::{QuantScaleTag, UnitRationalWire};
    use medulla_numeric::calibration::isotonic::fit_isotonic_weighted;

    fn policy() -> StatisticalPolicy {
        let rational = |n, d| UnitRationalWire::new(n, d).expect("rational");
        StatisticalPolicy {
            alpha: rational(1, 10),
            epsilon: rational(1, 10),
            delta: rational(1, 10),
            n_min: 20,
            min_support: 5,
            min_ess: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 10 << 32,
            },
            min_outcome_fidelity: medulla_decision_types::decision::TraceFidelityLevel::ToolCalls,
            tenant_public_features: false,
            audit_sample: rational(1, 20),
            approved_commit_principals: BoundedVec::new(vec!["approved".to_string()])
                .expect("inside bound"),
            compact_after_ms: None,
            drop_blob_after_ms: None,
        }
    }

    /// Two-option items: option 0 carries the feature, option 1 is the zero
    /// reference; the positive label is a noisy function of the feature.
    fn binary_examples(n: usize) -> Vec<Example> {
        (0..n)
            .map(|i| {
                let x = (i as f64 - (n as f64) / 2.0) / 10.0;
                let chance = 1.0 / (1.0 + (-x).exp());
                let positive = ((i * 7) % 10) as f64 / 10.0 < chance;
                let target = if positive { 1.0 } else { 0.0 };
                Example {
                    rows: vec![vec![x], vec![0.0]],
                    targets: vec![target, 1.0 - target],
                    weight: 1.0,
                }
            })
            .collect()
    }

    fn three_option_examples() -> Vec<Example> {
        (0..40)
            .map(|i| Example {
                rows: vec![vec![i as f64 / 10.0], vec![0.0], vec![1.0]],
                targets: vec![1.0, 0.0, 0.0],
                weight: 1.0,
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R032.2.1
    #[test]
    fn isotonic_calibration_is_monotone_and_reproducible() {
        let examples = binary_examples(80);
        let weights = [1.0];
        let fitted = calibrate(
            &examples,
            &weights,
            &policy(),
            false,
            Some(CalibrationMethod::Isotonic),
        )
        .expect("calibrates")
        .expect("held-out items exist");
        let table = fitted.isotonic.as_ref().expect("an isotonic table");
        table.check().expect("a monotone step function");
        let blocks = table.blocks.as_slice();
        assert!(!blocks.is_empty());
        assert!(
            blocks
                .windows(2)
                .all(|pair| pair[0].value.value <= pair[1].value.value
                    && pair[0].upper.value < pair[1].lower.value),
            "blocks are increasing with non-decreasing values"
        );

        // Reproducible: the same inputs give the same calibration.
        let again = calibrate(
            &examples,
            &weights,
            &policy(),
            false,
            Some(CalibrationMethod::Isotonic),
        )
        .expect("calibrates")
        .expect("held-out items exist");
        assert_eq!(fitted, again);

        // The served probability is the fitted map's prediction.
        let logits = logits_of(&examples, &weights);
        let scores: Vec<f64> = logits
            .iter()
            .map(|z| {
                let p = scaled_softmax(z, value_of(fitted.inverse_temperature)).expect("softmax");
                value_of(q32(p[0]).expect("quantises"))
            })
            .collect();
        let targets: Vec<f64> = examples.iter().map(|e| e.targets[0]).collect();
        let unit = vec![1.0; examples.len()];
        let reference = fit_isotonic_weighted(&scores, &targets, &unit).expect("fits");
        for score in &scores {
            let served = isotonic_value(table, *score);
            let predicted = reference.predict(*score).expect("predicts");
            assert!(
                (served - predicted).abs() < 1e-8,
                "served {served} equals predicted {predicted}"
            );
            let mapped = apply_isotonic(table, &[*score, 1.0 - *score]).expect("two options");
            assert!(
                (mapped[0] - served).abs() < 1e-12 && (mapped[0] + mapped[1] - 1.0).abs() < 1e-12
            );
        }
        // Monotone in the score.
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        let served: Vec<f64> = sorted.iter().map(|s| isotonic_value(table, *s)).collect();
        assert!(served.windows(2).all(|pair| pair[0] <= pair[1]));

        // The conformal threshold runs over the isotonic probabilities, and the
        // statement names the method.
        assert_eq!(
            crate::decision::head_eval::calibration_statement(&fitted).method,
            CalibrationMethod::Isotonic
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R032.2.1
    #[test]
    fn isotonic_refuses_more_than_two_options_and_unserved_methods() {
        let refused = calibrate(
            &three_option_examples(),
            &[1.0],
            &policy(),
            false,
            Some(CalibrationMethod::Isotonic),
        )
        .unwrap_err();
        assert_eq!(refused.code, "ISOTONIC_REQUIRES_BINARY");

        for method in [
            CalibrationMethod::Vector,
            CalibrationMethod::Dirichlet,
            CalibrationMethod::None,
        ] {
            let refused = calibrate(&binary_examples(40), &[1.0], &policy(), false, Some(method))
                .unwrap_err();
            assert_eq!(refused.code, "CALIBRATION_METHOD_UNSUPPORTED");
        }
        let kept = calibrate(&binary_examples(40), &[1.0], &policy(), false, None)
            .expect("calibrates")
            .expect("held-out items exist");
        assert!(
            kept.isotonic.is_none(),
            "no method keeps the ladder's choice"
        );
    }
}
