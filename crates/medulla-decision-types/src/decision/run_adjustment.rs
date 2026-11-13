//! Adjustment records (MEDULLA-DECISION-ENGINE-R158.1).
//!
//! A run's error rate over a window decides whether its model tier and width
//! change. These records carry the window and the adjustment with its reason.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;
use crate::decision::record::outcome::RefusalCode;
use medulla_types::contract::Digest256;

/// Abstention code: fewer runs than `width.min_runs`.
pub const INSUFFICIENT_SAMPLES: &str = "INSUFFICIENT_SAMPLES";
/// Refusal code: the window's partition has no child or operation.
pub const ERRORS_UNPARTITIONED: &str = "ERRORS_UNPARTITIONED";
/// Refusal code: `width.error_lo` exceeds `width.error_hi`.
pub const ADJUSTMENT_POLICY_UNDEFINED: &str = "ADJUSTMENT_POLICY_UNDEFINED";
/// Refusal code: the current width is above the host ceiling, or the ceiling is zero.
pub const HOST_CEILING_EXCEEDED: &str = "HOST_CEILING_EXCEEDED";
/// Refusal code: the current model is not among the declared tiers.
pub const CURRENT_MODEL_NOT_CANDIDATE: &str = "CURRENT_MODEL_NOT_CANDIDATE";

/// Abstention code: the window has no latency measurement.
pub const LATENCY_UNKNOWN: &str = "LATENCY_UNKNOWN";
/// Refusal code: a request made while a run is in flight would widen it.
pub const WIDEN_MID_RUN: &str = "WIDEN_MID_RUN";

/// The refusal and abstention codes the `run_adjustment` family declares.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(INSUFFICIENT_SAMPLES),
    RefusalCode::new(ERRORS_UNPARTITIONED),
    RefusalCode::new(ADJUSTMENT_POLICY_UNDEFINED),
    RefusalCode::new(HOST_CEILING_EXCEEDED),
    RefusalCode::new(CURRENT_MODEL_NOT_CANDIDATE),
    RefusalCode::new(LATENCY_UNKNOWN),
    RefusalCode::new(WIDEN_MID_RUN),
];

/// Error rate at or above which the model moves up and the width down.
pub const WIDTH_ERROR_HI: KnobSpec = KnobSpec {
    name: "width.error_hi",
    unit: "fraction",
    default: KnobValue::DecimalMicros(20_000),
    min: KnobValue::DecimalMicros(10_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};
/// Error rate at or below which the width grows.
pub const WIDTH_ERROR_LO: KnobSpec = KnobSpec {
    name: "width.error_lo",
    unit: "fraction",
    default: KnobValue::DecimalMicros(5_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(500_000),
    safety: KnobSafety::Tunable,
};
/// How many recent runs the window reads.
pub const WIDTH_WINDOW: KnobSpec = KnobSpec {
    name: "width.window",
    unit: "runs",
    default: KnobValue::Integer(20),
    min: KnobValue::Integer(5),
    max: KnobValue::Integer(200),
    safety: KnobSafety::Tunable,
};
/// The fewest runs an adjustment is decided on.
pub const WIDTH_MIN_RUNS: KnobSpec = KnobSpec {
    name: "width.min_runs",
    unit: "runs",
    default: KnobValue::Integer(10),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(200),
    safety: KnobSafety::Tunable,
};

/// The p95 run latency above which the width stops growing and drops.
pub const WIDTH_LATENCY_P95_CAP_MS: KnobSpec = KnobSpec {
    name: "width.latency_p95_cap_ms",
    unit: "ms",
    default: KnobValue::Integer(2_000),
    min: KnobValue::Integer(100),
    max: KnobValue::Integer(600_000),
    safety: KnobSafety::Tunable,
};
/// The oldest a run may be, by its newest record, and still be observed.
pub const WIDTH_WINDOW_MAX_AGE_S: KnobSpec = KnobSpec {
    name: "width.window_max_age_s",
    unit: "s",
    default: KnobValue::Integer(600),
    min: KnobValue::Integer(30),
    max: KnobValue::Integer(86_400),
    safety: KnobSafety::Tunable,
};

/// The `width.*` knobs.
pub const KNOBS: &[KnobSpec] = &[
    WIDTH_ERROR_HI,
    WIDTH_ERROR_LO,
    WIDTH_WINDOW,
    WIDTH_MIN_RUNS,
    WIDTH_LATENCY_P95_CAP_MS,
    WIDTH_WINDOW_MAX_AGE_S,
];

/// The resolved `width.*` knob values one decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidthKnobs {
    /// `width.error_hi` in millionths.
    pub error_hi_micros: i64,
    /// `width.error_lo` in millionths.
    pub error_lo_micros: i64,
    pub window: u32,
    pub min_runs: u32,
    /// `width.latency_p95_cap_ms`.
    pub latency_p95_cap_ms: u64,
    /// `width.window_max_age_s`.
    pub window_max_age_s: u64,
}

impl WidthKnobs {
    /// The values the policy pins, or the defaults.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        let count = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            error_hi_micros: micros(&WIDTH_ERROR_HI),
            error_lo_micros: micros(&WIDTH_ERROR_LO),
            window: count(&WIDTH_WINDOW),
            min_runs: count(&WIDTH_MIN_RUNS),
            latency_p95_cap_ms: u64::from(count(&WIDTH_LATENCY_P95_CAP_MS)),
            window_max_age_s: u64::from(count(&WIDTH_WINDOW_MAX_AGE_S)),
        }
    }
}

/// The model, width and budget a run uses now, and the largest width the host allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    pub model: String,
    pub width: u32,
    pub host_ceiling: u32,
    pub budget_tokens: u64,
    /// True when the request is made while a run is in flight; such a
    /// request may not widen.
    pub mid_run: bool,
}

/// What the observed runs say about latency and cost (MEDULLA-DECISION-ENGINE-R158.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    /// The 95th percentile of the window's per-run latency; `None` when no
    /// run reported one.
    pub latency_p95_ms: Option<u64>,
    /// The mean per-run cost of the window in micro-units; `None` when a run
    /// has no price.
    pub predicted_cost_micro: Option<u64>,
}

/// The 95th percentile (nearest rank) of per-run latencies; `None` for none.
pub fn latency_p95(mut latencies_ms: Vec<u64>) -> Option<u64> {
    if latencies_ms.is_empty() {
        return None;
    }
    latencies_ms.sort_unstable();
    let rank = (latencies_ms.len() * 95).div_ceil(100).max(1);
    Some(latencies_ms[rank - 1])
}

/// One legal model, ranked by its whole-number tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTier {
    pub model: String,
    pub tier: i64,
    /// What one run of this model costs relative to the others; 1 when the
    /// option declares none.
    pub cost_weight: u64,
}

/// Why no adjustment was decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdjustmentRefusal {
    /// `INSUFFICIENT_SAMPLES`: an abstention, not a request refusal.
    InsufficientSamples { runs: u32, min_runs: u32 },
    /// A request refusal named by its code.
    Invalid { code: &'static str, detail: String },
}

impl AdjustmentRefusal {
    /// The registered code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InsufficientSamples { .. } => INSUFFICIENT_SAMPLES,
            Self::Invalid { code, .. } => code,
        }
    }
}

/// The error rate, `errored_runs / runs`, in Q32.
pub fn error_rate_q32(window: &ErrorWindow) -> u64 {
    if window.runs == 0 {
        return 0;
    }
    ((u128::from(window.errored_runs) << 32) / u128::from(window.runs)) as u64
}

/// Decide the model and width of the next runs from one error window and the
/// latency and cost the same runs observed.
///
/// The pair is scored jointly: the error rate and the latency set a target
/// tier and a target width, and every declared model is scored by how far its
/// tier is from the target tier (the width is the target width, held in
/// `[1, host_ceiling]`). At or above `width.error_hi` the target is the lowest
/// tier above the current one and the width drops by one (never below 1). A
/// p95 latency above `width.latency_p95_cap_ms` drops the width by one and
/// keeps the model. At or below `width.error_lo` the width grows by one up to
/// the host ceiling. Otherwise nothing changes. On an equal score the lower
/// predicted cost (`predicted_cost_micro` x width x the model's cost weight)
/// wins, then the lower model IRI. An unknown latency abstains
/// (`LATENCY_UNKNOWN`); a request made mid-run never widens
/// (`WIDEN_MID_RUN`).
pub fn decide_adjustment(
    window: &ErrorWindow,
    observed: &Observed,
    current: &Current,
    tiers: &[ModelTier],
    knobs: &WidthKnobs,
) -> Result<ModelWidthAdjustment, AdjustmentRefusal> {
    let invalid = |code: &'static str, detail: &str| AdjustmentRefusal::Invalid {
        code,
        detail: detail.to_string(),
    };
    if window.child_id.is_empty() || window.operation.is_empty() {
        return Err(invalid(
            ERRORS_UNPARTITIONED,
            "child_id and operation must be set",
        ));
    }
    if knobs.error_lo_micros > knobs.error_hi_micros {
        return Err(invalid(
            ADJUSTMENT_POLICY_UNDEFINED,
            "width.error_lo exceeds width.error_hi",
        ));
    }
    if current.host_ceiling == 0 || current.width > current.host_ceiling {
        return Err(invalid(
            HOST_CEILING_EXCEEDED,
            "current width is above the host ceiling, or the ceiling is zero",
        ));
    }
    let Some(current_tier) = tiers.iter().find(|tier| tier.model == current.model) else {
        return Err(invalid(
            CURRENT_MODEL_NOT_CANDIDATE,
            "the current model is not among the declared options",
        ));
    };
    if window.runs < knobs.min_runs {
        return Err(AdjustmentRefusal::InsufficientSamples {
            runs: window.runs,
            min_runs: knobs.min_runs,
        });
    }
    let Some(latency_p95_ms) = observed.latency_p95_ms else {
        return Err(invalid(
            LATENCY_UNKNOWN,
            "no run in the window reported a latency",
        ));
    };
    // Compare `errored / runs` with a millionths threshold exactly.
    let scaled = i128::from(window.errored_runs) * 1_000_000;
    let runs = i128::from(window.runs);
    let high = scaled >= i128::from(knobs.error_hi_micros) * runs;
    let low = scaled <= i128::from(knobs.error_lo_micros) * runs;
    let slow = latency_p95_ms > knobs.latency_p95_cap_ms;

    let mut target_tier = current_tier.tier;
    let mut target_width = current.width;
    let rationale = if high {
        target_width = current.width.saturating_sub(1).max(1);
        let next = tiers
            .iter()
            .filter(|tier| tier.tier > current_tier.tier)
            .map(|tier| tier.tier)
            .min();
        match next {
            Some(tier) => {
                target_tier = tier;
                AdjustmentRationale::ErrorHigh
            }
            None => AdjustmentRationale::AtTopTier,
        }
    } else if slow {
        target_width = current.width.saturating_sub(1).max(1);
        AdjustmentRationale::LatencyHigh
    } else if low {
        if current.width >= current.host_ceiling {
            AdjustmentRationale::AtHostCeiling
        } else {
            target_width = current.width + 1;
            AdjustmentRationale::ErrorLow
        }
    } else {
        AdjustmentRationale::Unchanged
    };
    // Joint score: the tier distance of each model from the target (the
    // width is the target width for every model). Lowest distance, then lowest
    // predicted cost, then lowest model IRI wins.
    let predicted_cost = |tier: &ModelTier| {
        observed.predicted_cost_micro.map_or(u128::MAX, |cost| {
            u128::from(cost) * u128::from(target_width) * u128::from(tier.cost_weight)
        })
    };
    let chosen = tiers
        .iter()
        .min_by(|a, b| {
            (a.tier - target_tier)
                .unsigned_abs()
                .cmp(&(b.tier - target_tier).unsigned_abs())
                .then_with(|| predicted_cost(a).cmp(&predicted_cost(b)))
                .then_with(|| a.model.cmp(&b.model))
        })
        .unwrap_or(current_tier);
    // A model changes only when the tier does; at the current tier the
    // current model stays.
    let after_model = if target_tier == current_tier.tier {
        current.model.clone()
    } else {
        chosen.model.clone()
    };
    let after_width = target_width;
    if current.mid_run && after_width > current.width {
        return Err(invalid(
            WIDEN_MID_RUN,
            "a request made while a run is in flight may not widen it",
        ));
    }
    let basis = serde_json::json!({
        "error_rate_q32": error_rate_q32(window),
        "runs": window.runs,
        "errored_runs": window.errored_runs,
        "error_hi_micros": knobs.error_hi_micros,
        "error_lo_micros": knobs.error_lo_micros,
        "latency_p95_ms": latency_p95_ms,
        "latency_p95_cap_ms": knobs.latency_p95_cap_ms,
        "predicted_cost_micro": observed.predicted_cost_micro,
    });
    Ok(ModelWidthAdjustment {
        before_model: current.model.clone(),
        after_model,
        before_width: current.width,
        after_width,
        before_budget_tokens: current.budget_tokens,
        after_budget_tokens: current.budget_tokens,
        rationale,
        rationale_digest: Digest256::sha256(basis.to_string().as_bytes()),
    })
}

/// The partition of runs one error rate is computed over, with its counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ErrorWindow {
    pub tenant: String,
    pub child_id: String,
    pub operation: String,
    pub runs: u32,
    pub errored_runs: u32,
}

impl ErrorWindow {
    /// Errored runs never exceed runs.
    pub fn checked(self) -> Result<Self, String> {
        if self.errored_runs > self.runs {
            return Err("errored_runs exceeds runs".into());
        }
        Ok(self)
    }
}

/// Why an adjustment was or was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AdjustmentRationale {
    ErrorHigh,
    ErrorLow,
    Unchanged,
    AtTopTier,
    AtHostCeiling,
    /// The p95 latency is above `width.latency_p95_cap_ms`: the width drops.
    LatencyHigh,
}

/// A model and width change, with the rationale and its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModelWidthAdjustment {
    pub before_model: String,
    pub after_model: String,
    pub before_width: u32,
    pub after_width: u32,
    pub before_budget_tokens: u64,
    pub after_budget_tokens: u64,
    pub rationale: AdjustmentRationale,
    /// SHA-256 of the error rate, the window and the thresholds.
    pub rationale_digest: Digest256,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R158.1
    #[test]
    fn adjustment_types_round_trip() {
        let window = ErrorWindow {
            tenant: "t".into(),
            child_id: "child:1".into(),
            operation: "search".into(),
            runs: 20,
            errored_runs: 7,
        }
        .checked()
        .unwrap();
        let text = serde_json::to_string(&window).unwrap();
        assert_eq!(serde_json::from_str::<ErrorWindow>(&text).unwrap(), window);
        let adjustment = ModelWidthAdjustment {
            before_model: "small".into(),
            after_model: "large".into(),
            before_width: 2,
            after_width: 4,
            before_budget_tokens: 1_000,
            after_budget_tokens: 4_000,
            rationale: AdjustmentRationale::ErrorHigh,
            rationale_digest: Digest256::sha256(text.as_bytes()),
        };
        let text = serde_json::to_string(&adjustment).unwrap();
        assert!(text.contains("\"rationale\":\"error_high\""));
        assert_eq!(
            serde_json::from_str::<ModelWidthAdjustment>(&text).unwrap(),
            adjustment
        );
        for rationale in [
            AdjustmentRationale::ErrorHigh,
            AdjustmentRationale::ErrorLow,
            AdjustmentRationale::Unchanged,
            AdjustmentRationale::AtTopTier,
            AdjustmentRationale::AtHostCeiling,
        ] {
            let text = serde_json::to_string(&rationale).unwrap();
            assert_eq!(
                serde_json::from_str::<AdjustmentRationale>(&text).unwrap(),
                rationale
            );
        }
        assert!(ErrorWindow {
            errored_runs: 21,
            ..window.clone()
        }
        .checked()
        .is_err());
        let mut value = serde_json::to_value(&window).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ErrorWindow>(value).is_err());
    }

    fn window(runs: u32, errored: u32) -> ErrorWindow {
        ErrorWindow {
            tenant: "t".into(),
            child_id: "child:1".into(),
            operation: "search".into(),
            runs,
            errored_runs: errored,
        }
    }

    fn observed() -> Observed {
        Observed {
            latency_p95_ms: Some(500),
            predicted_cost_micro: Some(1_000),
        }
    }

    fn tiers() -> Vec<ModelTier> {
        vec![
            ModelTier {
                model: "small".into(),
                tier: 1,
                cost_weight: 1,
            },
            ModelTier {
                model: "medium".into(),
                tier: 2,
                cost_weight: 1,
            },
            ModelTier {
                model: "large".into(),
                tier: 3,
                cost_weight: 1,
            },
        ]
    }

    fn current(model: &str, width: u32, ceiling: u32) -> Current {
        Current {
            model: model.into(),
            width,
            host_ceiling: ceiling,
            budget_tokens: 1_000,
            mid_run: false,
        }
    }

    fn knobs() -> WidthKnobs {
        WidthKnobs {
            error_hi_micros: 20_000,
            error_lo_micros: 5_000,
            window: 20,
            min_runs: 10,
            latency_p95_cap_ms: 2_000,
            window_max_age_s: 600,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R158.2
    #[test]
    fn adjustment_rules() {
        // 0.03 (6 of 200 runs): up one tier, width down one.
        let high = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(high.rationale, AdjustmentRationale::ErrorHigh);
        assert_eq!((high.after_model.as_str(), high.after_width), ("medium", 3));
        // 0.01 (2 of 200): between the thresholds, nothing changes.
        let mid = decide_adjustment(
            &window(200, 2),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(mid.rationale, AdjustmentRationale::Unchanged);
        assert_eq!((mid.after_model.as_str(), mid.after_width), ("small", 4));
        // 0.004 (1 of 250): width up one, model kept.
        let low = decide_adjustment(
            &window(250, 1),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(low.rationale, AdjustmentRationale::ErrorLow);
        assert_eq!((low.after_model.as_str(), low.after_width), ("small", 5));
        // The top tier keeps its model and still lowers the width.
        let top = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("large", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(top.rationale, AdjustmentRationale::AtTopTier);
        assert_eq!((top.after_model.as_str(), top.after_width), ("large", 3));
        // Width never drops below 1.
        let floor = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("small", 1, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(floor.after_width, 1);
        // Width never exceeds the host ceiling.
        let ceiling = decide_adjustment(
            &window(250, 1),
            &observed(),
            &current("small", 8, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(ceiling.rationale, AdjustmentRationale::AtHostCeiling);
        assert_eq!(ceiling.after_width, 8);
        // Thresholds are inclusive.
        let exact = decide_adjustment(
            &window(100, 2),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(exact.rationale, AdjustmentRationale::ErrorHigh);
        // Equal inputs give an equal digest.
        let again = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(high.rationale_digest, again.rationale_digest);
        assert_eq!(error_rate_q32(&window(4, 1)), 1 << 30);

        // Refusals and the abstention.
        let few = decide_adjustment(
            &window(9, 9),
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap_err();
        assert_eq!(few.code(), INSUFFICIENT_SAMPLES);
        let mut unpartitioned = window(20, 1);
        unpartitioned.operation.clear();
        let refused = decide_adjustment(
            &unpartitioned,
            &observed(),
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap_err();
        assert_eq!(refused.code(), ERRORS_UNPARTITIONED);
        let inverted = WidthKnobs {
            error_lo_micros: 30_000,
            ..knobs()
        };
        assert_eq!(
            decide_adjustment(
                &window(20, 1),
                &observed(),
                &current("small", 4, 8),
                &tiers(),
                &inverted
            )
            .unwrap_err()
            .code(),
            ADJUSTMENT_POLICY_UNDEFINED
        );
        assert_eq!(
            decide_adjustment(
                &window(20, 1),
                &observed(),
                &current("small", 9, 8),
                &tiers(),
                &knobs()
            )
            .unwrap_err()
            .code(),
            HOST_CEILING_EXCEEDED
        );
        assert_eq!(
            decide_adjustment(
                &window(20, 1),
                &observed(),
                &current("small", 1, 0),
                &tiers(),
                &knobs()
            )
            .unwrap_err()
            .code(),
            HOST_CEILING_EXCEEDED
        );
        assert_eq!(
            decide_adjustment(
                &window(20, 1),
                &observed(),
                &current("ghost", 4, 8),
                &tiers(),
                &knobs()
            )
            .unwrap_err()
            .code(),
            CURRENT_MODEL_NOT_CANDIDATE
        );
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
        assert_eq!(
            WidthKnobs::from_policy(&DecisionPolicy::engine_default()),
            knobs()
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R158.7
    #[test]
    fn latency_cost_and_mid_run_rules() {
        let slow = Observed {
            latency_p95_ms: Some(2_001),
            predicted_cost_micro: Some(1_000),
        };
        // Quiet runs would widen, but a p95 above the cap drops the width.
        let capped = decide_adjustment(
            &window(250, 1),
            &slow,
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(capped.rationale, AdjustmentRationale::LatencyHigh);
        assert_eq!(
            (capped.after_model.as_str(), capped.after_width),
            ("small", 3)
        );
        // At the cap exactly the latency is fine.
        let at_cap = Observed {
            latency_p95_ms: Some(2_000),
            ..slow
        };
        let widened = decide_adjustment(
            &window(250, 1),
            &at_cap,
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap();
        assert_eq!(widened.after_width, 5);
        // An unknown latency abstains.
        let unknown = Observed {
            latency_p95_ms: None,
            ..slow
        };
        let refused = decide_adjustment(
            &window(250, 1),
            &unknown,
            &current("small", 4, 8),
            &tiers(),
            &knobs(),
        )
        .unwrap_err();
        assert_eq!(refused.code(), LATENCY_UNKNOWN);
        // A mid-run request may narrow or hold but never widen.
        let mut mid = current("small", 4, 8);
        mid.mid_run = true;
        let refused =
            decide_adjustment(&window(250, 1), &observed(), &mid, &tiers(), &knobs()).unwrap_err();
        assert_eq!(refused.code(), WIDEN_MID_RUN);
        let narrowed =
            decide_adjustment(&window(200, 6), &observed(), &mid, &tiers(), &knobs()).unwrap();
        assert_eq!(narrowed.after_width, 3);
        let held =
            decide_adjustment(&window(200, 2), &observed(), &mid, &tiers(), &knobs()).unwrap();
        assert_eq!(held.after_width, 4);
        // Two models share the next tier: the cheaper one wins, then the lower IRI.
        let mut shared = tiers();
        shared.push(ModelTier {
            model: "medium-b".into(),
            tier: 2,
            cost_weight: 1,
        });
        shared[1].cost_weight = 3;
        let cheaper = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("small", 4, 8),
            &shared,
            &knobs(),
        )
        .unwrap();
        assert_eq!(cheaper.after_model, "medium-b");
        shared[1].cost_weight = 1;
        let by_iri = decide_adjustment(
            &window(200, 6),
            &observed(),
            &current("small", 4, 8),
            &shared,
            &knobs(),
        )
        .unwrap();
        assert_eq!(by_iri.after_model, "medium");
        // An unknown cost leaves the order to the IRI.
        let no_cost = Observed {
            predicted_cost_micro: None,
            ..observed()
        };
        shared[1].cost_weight = 3;
        let unpriced = decide_adjustment(
            &window(200, 6),
            &no_cost,
            &current("small", 4, 8),
            &shared,
            &knobs(),
        )
        .unwrap();
        assert_eq!(unpriced.after_model, "medium");
        // The nearest-rank p95.
        assert_eq!(latency_p95(vec![]), None);
        assert_eq!(latency_p95((1..=20).collect()), Some(19));
        assert_eq!(latency_p95(vec![7]), Some(7));
    }
}
