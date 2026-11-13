//! The retrieval stop rule (MEDULLA-FEDERATED-QUERY-R082.2): one pure function that
//! decides, after a completed round, whether the loop stops.
//!
//! No I/O and no clock. Precedence: round limit, other exhausted budget, no new
//! evidence, quality saturation, then calibrated confidence. Progress comes from the
//! completed round's measured evidence, never from a caller or critique claim.

use medulla_result_types::retrieval_round::RoundAnswer;
use medulla_result_types::retrieval_stop::{
    CalibrationRefusal, CalibrationTable, ExhaustedDimension, StopContext, StopReason, StopVerdict,
};

/// What the completed round measured, for the two progress checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundProgress {
    /// The round returned no id that the earlier rounds had not returned.
    pub no_new_evidence: bool,
    /// The mean score moved by less than `knob retrieval.saturation_epsilon`.
    pub quality_saturated: bool,
}

/// Measure the progress of a completed round: the returned ids against the ids of
/// the earlier rounds, and the mean score of the returned rows against the prior
/// quality. A round with no prior quality cannot saturate.
pub fn round_progress(
    returned_ids: &[&str],
    prior_row_ids: &[String],
    quality: Option<f64>,
    prior_quality: Option<f64>,
    epsilon: f64,
) -> RoundProgress {
    let prior: std::collections::HashSet<&str> = prior_row_ids.iter().map(String::as_str).collect();
    let no_new_evidence = returned_ids.iter().all(|id| prior.contains(id));
    let quality_saturated = match (quality, prior_quality) {
        (Some(now), Some(before)) => (now - before).abs() < epsilon,
        _ => false,
    };
    RoundProgress {
        no_new_evidence,
        quality_saturated,
    }
}

fn exhausted(context: &StopContext) -> Option<ExhaustedDimension> {
    if context.round >= context.round_limit {
        Some(ExhaustedDimension::Round)
    } else if context.tokens_used >= context.token_limit {
        Some(ExhaustedDimension::Tokens)
    } else if context.inference_calls_used >= context.inference_limit {
        Some(ExhaustedDimension::InferenceCalls)
    } else if context.cost_used >= context.cost_limit {
        Some(ExhaustedDimension::Cost)
    } else {
        None
    }
}

/// Decide whether the loop stops after the completed round `context.round`.
///
/// The progress checks apply only when `context.completed_round_progress` says the
/// round supplied measured evidence. A confidence stop needs the last two answer
/// digests equal and `confidence - baseline_confidence > margin_threshold`; equality
/// at the threshold continues. A missing calibration row continues with
/// `ContinueUncalibrated` only when no earlier terminal condition applies, and stays
/// visible on a terminal verdict as `calibration_refusal`.
pub fn decide_stop(
    table: &CalibrationTable,
    history: &[RoundAnswer],
    context: &StopContext,
    progress: &RoundProgress,
) -> StopVerdict {
    let calibration = table.for_round(context.round);
    let verdict = |stop_reason: StopReason, dimension: Option<ExhaustedDimension>| StopVerdict {
        stop: stop_reason.is_terminal(),
        reason: stop_reason,
        round: context.round,
        margin: None,
        threshold: None,
        calibration_refusal: calibration
            .is_none()
            .then_some(CalibrationRefusal::RoundNotCalibrated),
        exhausted_dimension: dimension,
    };
    match exhausted(context) {
        Some(ExhaustedDimension::Round) => {
            return verdict(StopReason::RoundLimit, Some(ExhaustedDimension::Round))
        }
        Some(other) => return verdict(StopReason::BudgetExhausted, Some(other)),
        None => {}
    }
    if context.completed_round_progress {
        if progress.no_new_evidence {
            return verdict(StopReason::NoNewEvidence, None);
        }
        if progress.quality_saturated {
            return verdict(StopReason::QualitySaturated, None);
        }
    }
    let Some(row) = calibration else {
        return verdict(StopReason::ContinueUncalibrated, None);
    };
    let [.., previous, last] = history else {
        return verdict(StopReason::ContinueAnswerChanged, None);
    };
    if previous.answer_digest != last.answer_digest {
        return verdict(StopReason::ContinueAnswerChanged, None);
    }
    // The margin is reported on [0, 1]; a confidence below the baseline is margin 0.
    let margin = (last.confidence - row.baseline_confidence).clamp(0.0, 1.0);
    let reason = if last.confidence - row.baseline_confidence > row.margin_threshold {
        StopReason::AnswerRepeatedMarginMet
    } else {
        StopReason::ContinueMarginLow
    };
    StopVerdict {
        margin: Some(margin),
        threshold: Some(row.margin_threshold),
        ..verdict(reason, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::retrieval_stop::RoundCalibration;
    use medulla_types::contract::BoundedVec;

    fn table() -> CalibrationTable {
        CalibrationTable {
            rows: BoundedVec::new(vec![RoundCalibration {
                round: 2,
                baseline_confidence: 0.5,
                margin_threshold: 0.25,
            }])
            .expect("one row fits"),
        }
    }

    fn context(round: u32) -> StopContext {
        StopContext {
            round,
            round_limit: 4,
            token_limit: 1_000,
            tokens_used: 10,
            inference_limit: 8,
            inference_calls_used: 1,
            cost_limit: 1_000,
            cost_used: 10,
            completed_round_progress: true,
            policy_revision: "p1".into(),
        }
    }

    fn answer(round: u32, digest: &str, confidence: f64) -> RoundAnswer {
        RoundAnswer {
            round,
            answer_digest: digest.into(),
            confidence,
        }
    }

    const FRESH: RoundProgress = RoundProgress {
        no_new_evidence: false,
        quality_saturated: false,
    };

    #[test]
    fn repeat_and_margin_stops() {
        let t = table();
        let same = [answer(1, "a", 0.6), answer(2, "a", 0.8)];
        // Repeated answer with margin 0.30 > 0.25 stops.
        let met = decide_stop(&t, &same, &context(2), &FRESH);
        assert_eq!(met.reason, StopReason::AnswerRepeatedMarginMet);
        assert!(met.stop && met.validate().is_ok());
        assert_eq!(met.threshold, Some(0.25));
        // Equality at the threshold continues.
        let equal = [answer(1, "a", 0.6), answer(2, "a", 0.75)];
        let low = decide_stop(&t, &equal, &context(2), &FRESH);
        assert_eq!(low.reason, StopReason::ContinueMarginLow);
        assert!(!low.stop && low.validate().is_ok());
        // A changed answer, or one answer only, continues.
        let changed = [answer(1, "a", 0.9), answer(2, "b", 0.9)];
        assert_eq!(
            decide_stop(&t, &changed, &context(2), &FRESH).reason,
            StopReason::ContinueAnswerChanged
        );
        assert_eq!(
            decide_stop(&t, &same[..1], &context(2), &FRESH).reason,
            StopReason::ContinueAnswerChanged
        );
        // A negative margin is reported as 0.
        let below = [answer(1, "a", 0.1), answer(2, "a", 0.1)];
        let v = decide_stop(&t, &below, &context(2), &FRESH);
        assert_eq!(
            (v.reason, v.margin),
            (StopReason::ContinueMarginLow, Some(0.0))
        );
        assert!(v.validate().is_ok());

        // No calibration row: continue uncalibrated only when nothing terminal applies.
        let none = decide_stop(&t, &same, &context(1), &FRESH);
        assert_eq!(none.reason, StopReason::ContinueUncalibrated);
        assert_eq!(
            none.calibration_refusal,
            Some(CalibrationRefusal::RoundNotCalibrated)
        );
        assert!(none.validate().is_ok());

        // Precedence with simultaneous conditions: round limit, other budget, no new
        // evidence, saturation, then confidence.
        let all = RoundProgress {
            no_new_evidence: true,
            quality_saturated: true,
        };
        let mut at_limit = context(4);
        at_limit.tokens_used = 1_000;
        assert_eq!(
            decide_stop(&t, &same, &at_limit, &all).reason,
            StopReason::RoundLimit
        );
        let mut budget = context(2);
        budget.inference_calls_used = 8;
        let b = decide_stop(&t, &same, &budget, &all);
        assert_eq!(
            (b.reason, b.exhausted_dimension),
            (
                StopReason::BudgetExhausted,
                Some(ExhaustedDimension::InferenceCalls)
            )
        );
        assert_eq!(
            decide_stop(&t, &same, &context(2), &all).reason,
            StopReason::NoNewEvidence
        );
        let sat = RoundProgress {
            no_new_evidence: false,
            quality_saturated: true,
        };
        assert_eq!(
            decide_stop(&t, &same, &context(2), &sat).reason,
            StopReason::QualitySaturated
        );
        // Progress claims are ignored when the round supplied no measured evidence.
        let mut unmeasured = context(2);
        unmeasured.completed_round_progress = false;
        assert_eq!(
            decide_stop(&t, &same, &unmeasured, &all).reason,
            StopReason::AnswerRepeatedMarginMet
        );
        // A terminal verdict keeps the missing calibration visible.
        let terminal = decide_stop(&t, &same, &at_limit, &FRESH);
        assert_eq!(
            terminal.calibration_refusal,
            Some(CalibrationRefusal::RoundNotCalibrated)
        );
        assert!(terminal.validate().is_ok());
    }

    #[test]
    fn progress_measures_new_ids_and_saturation() {
        let prior = vec!["a".to_string(), "b".to_string()];
        let same = round_progress(&["a", "b"], &prior, Some(0.50), Some(0.505), 0.01);
        assert!(same.no_new_evidence && same.quality_saturated);
        let fresh = round_progress(&["a", "c"], &prior, Some(0.9), Some(0.5), 0.01);
        assert!(!fresh.no_new_evidence && !fresh.quality_saturated);
        // No prior quality cannot saturate; equality at epsilon does not saturate.
        assert!(!round_progress(&["a"], &prior, Some(0.5), None, 0.01).quality_saturated);
        assert!(!round_progress(&["a"], &prior, Some(0.75), Some(0.5), 0.25).quality_saturated);
    }
}
