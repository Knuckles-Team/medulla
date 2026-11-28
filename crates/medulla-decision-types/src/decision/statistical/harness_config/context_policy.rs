//! Pure context-pressure selection (MEDULLA-DECISION-ENGINE-R128.5).

use super::{ContextPolicy, HarnessPart};
use medulla_decision_statistical_types::decision::numeric::UnitRationalWire;
use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

const Q32: u64 = 1 << 32;

/// Rule thresholds, passed explicitly after the caller resolves its policy.
/// Tenant-policy lookup and premise sealing remain R128.4/R128.11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessDefaults {
    pub soft_threshold: UnitRationalWire,
    pub hard_threshold: UnitRationalWire,
    pub recent_window_fraction: UnitRationalWire,
}

impl Default for HarnessDefaults {
    fn default() -> Self {
        let fraction = |numerator, denominator| {
            UnitRationalWire::new(Q32 * numerator / denominator, Q32)
                .expect("default fractions are within one")
        };
        Self {
            soft_threshold: fraction(3, 5),
            hard_threshold: fraction(17, 20),
            recent_window_fraction: fraction(3, 10),
        }
    }
}

/// Refusal code of a threshold set that fails validation
/// (MEDULLA-DECISION-ENGINE-R128.4); the wire error is `PARAMETER_INVALID`.
pub const HARNESS_THRESHOLDS_INVALID: &str = "HARNESS_THRESHOLDS_INVALID";

const fn fraction_knob(name: &'static str, default: i64, min: i64, max: i64) -> KnobSpec {
    KnobSpec {
        name,
        unit: "fraction",
        default: KnobValue::DecimalMicros(default),
        min: KnobValue::DecimalMicros(min),
        max: KnobValue::DecimalMicros(max),
        safety: KnobSafety::Tunable,
    }
}

/// The `harness.*` threshold knobs (MEDULLA-DECISION-ENGINE-R128.4), in the
/// field order of [`HarnessDefaults`]: soft, hard, recent window.
pub const KNOBS: &[KnobSpec] = &[
    fraction_knob("harness.soft_threshold", 600_000, 50_000, 950_000),
    fraction_knob("harness.hard_threshold", 850_000, 100_000, 990_000),
    fraction_knob("harness.recent_window_fraction", 300_000, 50_000, 900_000),
];

/// `round(micros / 10^6 * 2^32)` as a Q32 fraction.
fn q32_fraction(micros: i64) -> Result<UnitRationalWire, String> {
    let micros = u128::try_from(micros)
        .map_err(|_| format!("{HARNESS_THRESHOLDS_INVALID}: a threshold knob is negative"))?;
    let numerator = (micros * u128::from(Q32) + 500_000) / 1_000_000;
    let numerator = u64::try_from(numerator)
        .map_err(|_| format!("{HARNESS_THRESHOLDS_INVALID}: a threshold knob overflows Q32"))?;
    UnitRationalWire::new(numerator, Q32)
        .map_err(|detail| format!("{HARNESS_THRESHOLDS_INVALID}: {detail}"))
}

impl HarnessDefaults {
    /// The thresholds the pinned policy sets, each knob or its default, as
    /// exact Q32 fractions (MEDULLA-DECISION-ENGINE-R128.4). A set that fails
    /// [`HarnessDefaults::validate`] (soft not below hard) refuses with
    /// `HARNESS_THRESHOLDS_INVALID`.
    pub fn from_policy(policy: &DecisionPolicy) -> Result<Self, String> {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DecimalMicros(micros) => q32_fraction(micros),
            _ => Err(format!(
                "{HARNESS_THRESHOLDS_INVALID}: {} has the wrong value type",
                KNOBS[index].name
            )),
        };
        let defaults = Self {
            soft_threshold: read(0)?,
            hard_threshold: read(1)?,
            recent_window_fraction: read(2)?,
        };
        defaults
            .validate()
            .map_err(|detail| format!("{HARNESS_THRESHOLDS_INVALID}: {detail}"))?;
        Ok(defaults)
    }

    /// All thresholds use Q32; soft precedes hard, and each fraction is positive.
    pub fn validate(&self) -> Result<(), String> {
        for fraction in [
            self.soft_threshold,
            self.hard_threshold,
            self.recent_window_fraction,
        ] {
            if fraction.denominator() != Q32 || fraction.numerator() == 0 {
                return Err("harness thresholds require positive Q32 fractions".into());
            }
        }
        if self.soft_threshold.numerator() >= self.hard_threshold.numerator() {
            return Err("harness soft threshold must precede hard threshold".into());
        }
        Ok(())
    }
}

/// Pressure and the exact thresholds used alongside the selected policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPolicyDecision {
    pub part: HarnessPart<ContextPolicy>,
    /// `floor(expected_trajectory_tokens * 2^32 / context_window_tokens)`.
    /// Unsigned storage covers the full u32 token range, including pressure > 1.
    pub pressure_q32: Option<u64>,
    pub thresholds: HarnessDefaults,
}

/// Resolve context handling without treating a point estimate as a hard bound.
/// `task_token_upper_bound` must be an enforced bound on the complete context
/// for the task class, including prompt, generated and tool tokens. A measured
/// maximum or an expected trajectory is not such a bound. Its source and
/// enforcement are the caller's responsibility; absent it, retain compaction.
pub fn decide_context_policy(
    context_window_tokens: Option<u32>,
    expected_trajectory_tokens: Option<u32>,
    task_token_upper_bound: Option<u32>,
    thresholds: &HarnessDefaults,
) -> Result<ContextPolicyDecision, String> {
    thresholds.validate()?;
    if context_window_tokens == Some(0) {
        return Err("context window must be non-zero".into());
    }
    if let (Some(expected), Some(bound)) = (expected_trajectory_tokens, task_token_upper_bound) {
        if expected > bound {
            return Err("trajectory estimate exceeds the task token upper bound".into());
        }
    }
    match (context_window_tokens, expected_trajectory_tokens) {
        (Some(window), Some(expected)) => Ok(ContextPolicyDecision {
            part: HarnessPart::Decided {
                value: if task_token_upper_bound.is_some_and(|bound| bound <= window) {
                    ContextPolicy::None
                } else {
                    ContextPolicy::ElideThenSummarize
                },
            },
            pressure_q32: Some((u64::from(expected) << 32) / u64::from(window)),
            thresholds: thresholds.clone(),
        }),
        _ => Ok(missing_context_inputs(
            context_window_tokens,
            expected_trajectory_tokens,
            thresholds,
        )),
    }
}

fn missing_context_inputs(
    window: Option<u32>,
    trajectory: Option<u32>,
    thresholds: &HarnessDefaults,
) -> ContextPolicyDecision {
    let mut missing = Vec::new();
    if window.is_none() {
        missing.push("contextWindowTokens".into());
    }
    if trajectory.is_none() {
        missing.push("expected_trajectory_tokens".into());
    }
    ContextPolicyDecision {
        part: HarnessPart::Abstained {
            missing_fact: BoundedVec::new(missing).expect("at most two missing facts"),
            conservative_default: ContextPolicy::ElideThenSummarize,
        },
        pressure_q32: None,
        thresholds: thresholds.clone(),
    }
}

#[cfg(test)]
mod tests;
