//! Serving technique activation (MEDULLA-DECISION-ENGINE-R819).
//!
//! A serving technique (a quantisation, a speculative-decoding setup, a cache
//! policy) is activated for one model and configuration only when every
//! benchmark receipt names exactly that technique, model and configuration
//! digest, the candidate metric regresses at most `serving.max_regression`
//! against the baseline, and it gains at least `serving.min_gain`. All
//! comparisons are integer parts-per-million. Activation is advisory and
//! `Decide` writes nothing.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;

/// Text parameter: the technique asked about.
pub const PARAM_TECHNIQUE: &str = "technique";
/// Text parameter: the model the technique would serve.
pub const PARAM_MODEL: &str = "model";
/// Text parameter: the digest of the serving configuration.
pub const PARAM_CONFIG_DIGEST: &str = "config_digest";
/// JSON text parameter: the benchmark receipts, an array of [`ServingReceipt`].
pub const PARAM_RECEIPTS: &str = "receipts";
/// The one option a serving technique question declares.
pub const ACTIVATION_OPTION: &str = "activation";
/// The answer that activates the technique.
pub const ACTIVATE: &str = "activate";
/// The answer that keeps the current serving setup.
pub const HOLD: &str = "hold";

/// One benchmark receipt: a technique measured against a baseline. Metrics are
/// higher-is-better, in parts per million of the metric's own scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingReceipt {
    pub technique: String,
    pub model: String,
    pub config_digest: String,
    pub baseline_ppm: u64,
    pub candidate_ppm: u64,
}

/// The decision-policy knobs of serving activation (DE-R059.1-KNOBS).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "serving.max_regression",
        unit: "fraction",
        default: KnobValue::DecimalMicros(0),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(100_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "serving.min_gain",
        unit: "fraction",
        default: KnobValue::DecimalMicros(0),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

fn micros(policy: &DecisionPolicy, spec: &KnobSpec) -> u64 {
    match policy.knob(spec) {
        KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(0),
        _ => 0,
    }
}

/// The most the candidate may regress, as a fraction in parts per million.
pub fn max_regression_ppm(policy: &DecisionPolicy) -> u64 {
    micros(policy, &KNOBS[0])
}

/// The least the candidate must gain, as a fraction in parts per million.
pub fn min_gain_ppm(policy: &DecisionPolicy) -> u64 {
    micros(policy, &KNOBS[1])
}

/// The family's refusal table, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("TECHNIQUE_EVIDENCE_MISSING"),
    RefusalCode::new("TECHNIQUE_EVIDENCE_MISMATCH"),
    RefusalCode::new("TECHNIQUE_REGRESSED"),
];

/// `TECHNIQUE_EVIDENCE_MISSING`.
pub const EVIDENCE_MISSING: RefusalCode = REFUSALS[0];
/// `TECHNIQUE_EVIDENCE_MISMATCH`.
pub const EVIDENCE_MISMATCH: RefusalCode = REFUSALS[1];
/// `TECHNIQUE_REGRESSED`.
pub const REGRESSED: RefusalCode = REFUSALS[2];

/// What the question asks about.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    pub technique: &'a str,
    pub model: &'a str,
    pub config_digest: &'a str,
}

/// Why a verdict cannot be reached; the code and the detail of the refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

/// The verdict: `activate` or `hold`, or the refusal.
pub fn verdict(
    subject: Subject<'_>,
    receipts: &[ServingReceipt],
    max_regression_ppm: u64,
    min_gain_ppm: u64,
) -> Result<&'static str, ServingRefusal> {
    if receipts.is_empty() {
        return Err(ServingRefusal {
            code: EVIDENCE_MISSING,
            detail: "no benchmark receipt was given".to_string(),
        });
    }
    if let Some(receipt) = receipts.iter().find(|r| {
        r.technique != subject.technique
            || r.model != subject.model
            || r.config_digest != subject.config_digest
    }) {
        return Err(ServingRefusal {
            code: EVIDENCE_MISMATCH,
            detail: format!(
                "a receipt names technique `{}` on model `{}`, not the one asked about",
                receipt.technique, receipt.model
            ),
        });
    }
    let mut gains = true;
    for receipt in receipts {
        let baseline = u128::from(receipt.baseline_ppm);
        let candidate = u128::from(receipt.candidate_ppm);
        if candidate < baseline
            && (baseline - candidate) * 1_000_000 > u128::from(max_regression_ppm) * baseline
        {
            return Err(ServingRefusal {
                code: REGRESSED,
                detail: format!(
                    "the candidate scored {candidate} ppm against a baseline of {baseline} ppm"
                ),
            });
        }
        if candidate < baseline
            || (candidate - baseline) * 1_000_000 < u128::from(min_gain_ppm) * baseline
        {
            gains = false;
        }
    }
    Ok(if gains { ACTIVATE } else { HOLD })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(baseline_ppm: u64, candidate_ppm: u64) -> ServingReceipt {
        ServingReceipt {
            technique: "spec-decode".into(),
            model: "m".into(),
            config_digest: "d".into(),
            baseline_ppm,
            candidate_ppm,
        }
    }

    fn subject() -> Subject<'static> {
        Subject {
            technique: "spec-decode",
            model: "m",
            config_digest: "d",
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R819
    #[test]
    fn verdict_boundaries_follow_the_two_knobs() {
        assert_eq!(
            verdict(subject(), &[], 0, 0).unwrap_err().code,
            EVIDENCE_MISSING
        );
        let mut other = receipt(1_000, 1_100);
        other.model = "x".into();
        assert_eq!(
            verdict(subject(), &[other], 0, 0).unwrap_err().code,
            EVIDENCE_MISMATCH
        );
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 1_000)], 0, 0),
            Ok(ACTIVATE)
        );
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 999)], 0, 0)
                .unwrap_err()
                .code,
            REGRESSED
        );
        // A 5% tolerance admits a 4% regression and refuses a 6% one.
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 960)], 50_000, 0),
            Ok(HOLD)
        );
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 940)], 50_000, 0)
                .unwrap_err()
                .code,
            REGRESSED
        );
        // A 10% floor holds a 9% gain and activates a 10% gain.
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 1_090)], 0, 100_000),
            Ok(HOLD)
        );
        assert_eq!(
            verdict(subject(), &[receipt(1_000, 1_100)], 0, 100_000),
            Ok(ACTIVATE)
        );
        // Every receipt must hold.
        assert_eq!(
            verdict(
                subject(),
                &[receipt(1_000, 1_200), receipt(1_000, 1_000)],
                0,
                100_000
            ),
            Ok(HOLD)
        );
    }
}
