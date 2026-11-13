//! The delegation quality model (MEDULLA-DECISION-ENGINE-R157.1).
//!
//! A delegate is admitted when enough recent observations of its work score at
//! or above a threshold. Scores and thresholds are integer micro-units in
//! `[0, 1000000]`; times are Unix milliseconds.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::record::outcome::RefusalCode;
use crate::decision::request::DecisionPolicyRef;
use crate::decision::statistical::StatisticalErrorCode;
use medulla_types::contract::{BoundedVec, Digest256};

/// The largest micro-unit score.
pub const MICRO_ONE: u32 = 1_000_000;

/// The `delegation.*` knobs: `min_observations`, `quality_min`, `min_window`,
/// then `window_max_age`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "delegation.min_observations",
        unit: "observations",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "delegation.quality_min",
        unit: "micro-score",
        default: KnobValue::Integer(800_000),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1_000_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "delegation.min_window",
        unit: "seconds",
        default: KnobValue::Integer(3_600),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(2_592_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "delegation.window_max_age",
        unit: "seconds",
        default: KnobValue::Integer(86_400),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(604_800),
        safety: KnobSafety::Tunable,
    },
];

/// One scored observation of a delegate's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityObservation {
    pub observation_id: String,
    pub score: u32,
    pub observed_at_ms: u64,
    pub evidence_digest: Digest256,
}

/// The time window the observations cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationQualityWindow {
    pub start_ms: u64,
    pub end_ms: u64,
    pub observation_count: u32,
}

/// What a delegate must reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityThreshold {
    pub metric_name: String,
    pub minimum_value: u32,
    pub min_observations: u32,
}

/// The admission verdict and the evidence it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationAdmission {
    pub admitted: bool,
    pub reason: String,
    pub quality_value: u32,
    pub quality_evidence_digest: Digest256,
}

impl QualityObservation {
    /// The score is within `[0, 1000000]`.
    pub fn checked(self) -> Result<Self, String> {
        if self.score > MICRO_ONE {
            return Err(format!("score {} exceeds {MICRO_ONE}", self.score));
        }
        Ok(self)
    }
}

impl DelegationQualityWindow {
    /// The window is not inverted.
    pub fn checked(self) -> Result<Self, String> {
        if self.start_ms > self.end_ms {
            return Err("window start is after its end".into());
        }
        Ok(self)
    }
}

/// SHA-256 over the ordered `observation_id` and `evidence_digest` pairs.
pub fn evidence_digest(observations: &[QualityObservation]) -> Digest256 {
    let mut fields: Vec<Vec<u8>> = Vec::new();
    for observation in observations {
        fields.push(observation.observation_id.as_bytes().to_vec());
        fields.push(observation.evidence_digest.as_bytes().to_vec());
    }
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    Digest256::framed(b"eg/delegation-quality-evidence", &refs)
        .unwrap_or_else(|_| Digest256::sha256(b""))
}

/// The reason an unadmitted delegation carries.
pub const DELEGATION_BELOW_THRESHOLD: &str = "DELEGATION_BELOW_THRESHOLD";
/// The reason an admitted delegation carries.
pub const DELEGATION_ADMITTED: &str = "quality at or above threshold";

/// The resolved `delegation.*` values an absent threshold falls back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegationKnobs {
    pub quality_min: u32,
    pub min_observations: u32,
    /// `delegation.min_window`, in seconds.
    pub min_window_s: u32,
    /// `delegation.window_max_age`, in seconds.
    pub window_max_age_s: u32,
}

impl DelegationKnobs {
    /// The values the policy pins, or the defaults.
    pub fn from_policy(policy: &crate::decision::policy::DecisionPolicy) -> Self {
        let value = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            quality_min: value(&KNOBS[1]),
            min_observations: value(&KNOBS[0]),
            min_window_s: value(&KNOBS[2]),
            window_max_age_s: value(&KNOBS[3]),
        }
    }
}

/// Admit a delegate: keep the observations inside `[start_ms, end_ms]`, take
/// their integer mean (rounded toward zero) as quality, and admit when their
/// count reaches `min_observations` and quality reaches `minimum_value`. An
/// absent threshold takes the `delegation.*` knob values.
pub fn admit_delegation(
    window: &DelegationQualityWindow,
    observations: &[QualityObservation],
    threshold: Option<&QualityThreshold>,
    knobs: &DelegationKnobs,
) -> DelegationAdmission {
    let kept: Vec<QualityObservation> = observations
        .iter()
        .filter(|o| o.observed_at_ms >= window.start_ms && o.observed_at_ms <= window.end_ms)
        .cloned()
        .collect();
    let (minimum_value, min_observations) = match threshold {
        Some(t) => (t.minimum_value, t.min_observations),
        None => (knobs.quality_min, knobs.min_observations),
    };
    let quality_value = if kept.is_empty() {
        0
    } else {
        let sum: u64 = kept.iter().map(|o| u64::from(o.score)).sum();
        (sum / kept.len() as u64) as u32
    };
    let admitted =
        kept.len() as u64 >= u64::from(min_observations) && quality_value >= minimum_value;
    DelegationAdmission {
        admitted,
        reason: if admitted {
            DELEGATION_ADMITTED.to_string()
        } else {
            DELEGATION_BELOW_THRESHOLD.to_string()
        },
        quality_value,
        quality_evidence_digest: evidence_digest(&kept),
    }
}

/// Whether a new submission replaces the previous window of the same
/// delegation scope: compare-and-set on `end_ms`, so a submission that ends
/// before the stored window does not replace it.
pub fn replaces_window(
    previous: Option<&DelegationQualityWindow>,
    next: &DelegationQualityWindow,
) -> bool {
    previous.is_none_or(|stored| next.end_ms >= stored.end_ms)
}

/// Refusal code: the window ends before `now - delegation.window_max_age`
/// (MEDULLA-DECISION-ENGINE-R157.4).
pub const DELEGATION_WINDOW_STALE: RefusalCode = RefusalCode::new("DELEGATION_WINDOW_STALE");
/// Refusal code: the window is shorter than `delegation.min_window`.
pub const DELEGATION_WINDOW_UNSTABLE: RefusalCode = RefusalCode::new("DELEGATION_WINDOW_UNSTABLE");
/// Refusal code: the threshold names an unknown metric or a value outside
/// `[0, 1000000]`.
pub const DELEGATION_THRESHOLD_UNDEFINED: RefusalCode =
    RefusalCode::new("DELEGATION_THRESHOLD_UNDEFINED");
/// Refusal code: fewer observations in the window than the minimum.
pub const DELEGATION_INSUFFICIENT_SAMPLES: RefusalCode =
    RefusalCode::new("DELEGATION_INSUFFICIENT_SAMPLES");

/// The delegation quality refusals, registered with the decision refusal
/// registry (`codes::module_tables`); each is also a `StatisticalErrorCode`.
pub const REFUSALS: &[RefusalCode] = &[
    DELEGATION_WINDOW_STALE,
    DELEGATION_WINDOW_UNSTABLE,
    DELEGATION_THRESHOLD_UNDEFINED,
    DELEGATION_INSUFFICIENT_SAMPLES,
];

/// The quality metrics a threshold may name: the closed set of measures a
/// delegate's observations score.
pub const METRICS: &[&str] = &["task_success", "review_accepted", "defect_free"];

/// The most observations one submission carries (the largest
/// `delegation.min_observations`).
pub const MAX_OBSERVATIONS: usize = 10_000;

/// The widest delegation scope name, in bytes.
pub const MAX_SCOPE_BYTES: usize = 256;

/// The key family of the sealed quality artifact of one delegation scope.
pub fn artifact_key(scope: &str) -> String {
    format!("delegation:{scope}")
}

/// Submit one observation window for a delegation scope
/// (`DecisionEval` op `delegation_quality`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationQualityRequest {
    pub tenant_id: String,
    /// The delegation the observations judge: the key of its sealed artifact.
    pub scope: String,
    /// The pinned policy whose `delegation.*` knobs apply.
    pub policy: DecisionPolicyRef,
    pub window: DelegationQualityWindow,
    pub observations: BoundedVec<QualityObservation, MAX_OBSERVATIONS>,
    #[serde(default)]
    pub threshold: Option<QualityThreshold>,
}

/// The sealed quality artifact of one delegation scope: the window and
/// threshold judged and the admission they produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StoredDelegation {
    pub scope: String,
    pub window: DelegationQualityWindow,
    pub threshold: Option<QualityThreshold>,
    pub admission: DelegationAdmission,
}

/// What the job answers: the authoritative stored artifact of the scope, its
/// digest, and whether this submission replaced the previous window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationQualityOutcome {
    pub stored: StoredDelegation,
    pub artifact_digest: Digest256,
    pub replaced: bool,
}

/// Judge a submission before the gate runs: the request refusals of
/// MEDULLA-DECISION-ENGINE-R157.4, plus malformed input. `now_ms` is the
/// authoritative clock. Returns the refusal text (`"CODE: detail"`).
pub fn check_request(
    request: &DelegationQualityRequest,
    knobs: &DelegationKnobs,
    now_ms: u64,
) -> Result<(), String> {
    let invalid = |detail: &str| StatisticalErrorCode::ParameterInvalid.refusal(detail);
    medulla_types::contract::bounded_text_or(&request.scope, MAX_SCOPE_BYTES, ())
        .map_err(|()| invalid("scope must be non-empty text without control characters"))?;
    request
        .window
        .checked()
        .map_err(|detail| invalid(&detail))?;
    for observation in &request.observations {
        observation
            .clone()
            .checked()
            .map_err(|detail| invalid(&detail))?;
    }
    let mut seen = std::collections::BTreeSet::new();
    if !request
        .observations
        .iter()
        .all(|observation| seen.insert(observation.observation_id.as_str()))
    {
        return Err(invalid("observation ids must be distinct"));
    }
    if u64::from(request.window.observation_count) != request.observations.len() as u64 {
        return Err(invalid(
            "window.observation_count must equal the observations",
        ));
    }
    if request.window.end_ms > now_ms {
        return Err(invalid("a window cannot end in the future"));
    }
    if let Some(threshold) = &request.threshold {
        if !METRICS.contains(&threshold.metric_name.as_str()) {
            return Err(StatisticalErrorCode::DelegationThresholdUndefined
                .refusal(format!("unknown metric {}", threshold.metric_name)));
        }
        if threshold.minimum_value > MICRO_ONE {
            return Err(
                StatisticalErrorCode::DelegationThresholdUndefined.refusal(format!(
                    "minimum_value {} is outside [0, {MICRO_ONE}]",
                    threshold.minimum_value
                )),
            );
        }
    }
    let span_ms = request.window.end_ms - request.window.start_ms;
    if span_ms < u64::from(knobs.min_window_s) * 1_000 {
        return Err(
            StatisticalErrorCode::DelegationWindowUnstable.refusal(format!(
                "the window spans {span_ms} ms, under delegation.min_window {} s",
                knobs.min_window_s
            )),
        );
    }
    if now_ms - request.window.end_ms > u64::from(knobs.window_max_age_s) * 1_000 {
        return Err(StatisticalErrorCode::DelegationWindowStale.refusal(format!(
            "the window ended at {} ms, older than delegation.window_max_age {} s",
            request.window.end_ms, knobs.window_max_age_s
        )));
    }
    let in_window = request
        .observations
        .iter()
        .filter(|o| {
            o.observed_at_ms >= request.window.start_ms && o.observed_at_ms <= request.window.end_ms
        })
        .count() as u64;
    let minimum = request
        .threshold
        .as_ref()
        .map_or(knobs.min_observations, |t| t.min_observations);
    if in_window < u64::from(minimum) {
        return Err(
            StatisticalErrorCode::DelegationInsufficientSamples.refusal(format!(
                "{in_window} observations in the window, fewer than {minimum}"
            )),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(id: &str, score: u32) -> QualityObservation {
        QualityObservation {
            observation_id: id.into(),
            score,
            observed_at_ms: 1_700_000_000_000,
            evidence_digest: Digest256::sha256(id.as_bytes()),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R157.1
    #[test]
    fn types_round_trip() {
        let observations = vec![observation("o1", 900_000), observation("o2", 700_000)];
        let admission = DelegationAdmission {
            admitted: true,
            reason: "quality at or above threshold".into(),
            quality_value: 800_000,
            quality_evidence_digest: evidence_digest(&observations),
        };
        let window = DelegationQualityWindow {
            start_ms: 1,
            end_ms: 2,
            observation_count: 2,
        };
        let threshold = QualityThreshold {
            metric_name: "task_success".into(),
            minimum_value: 800_000,
            min_observations: 20,
        };
        for observation in &observations {
            let text = serde_json::to_string(observation).unwrap();
            assert_eq!(
                &serde_json::from_str::<QualityObservation>(&text).unwrap(),
                observation
            );
        }
        let text = serde_json::to_string(&admission).unwrap();
        assert_eq!(
            serde_json::from_str::<DelegationAdmission>(&text).unwrap(),
            admission
        );
        let text = serde_json::to_string(&window).unwrap();
        assert_eq!(
            serde_json::from_str::<DelegationQualityWindow>(&text).unwrap(),
            window
        );
        let text = serde_json::to_string(&threshold).unwrap();
        assert_eq!(
            serde_json::from_str::<QualityThreshold>(&text).unwrap(),
            threshold
        );

        // The evidence digest depends on the order and content of the pairs.
        let reversed = vec![observation("o2", 700_000), observation("o1", 900_000)];
        assert_ne!(evidence_digest(&observations), evidence_digest(&reversed));
        assert!(observation("o3", 1_000_001).checked().is_err());
        assert!(observation("o3", 1_000_000).checked().is_ok());
        assert!(DelegationQualityWindow {
            start_ms: 3,
            end_ms: 2,
            observation_count: 0
        }
        .checked()
        .is_err());
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
    }

    fn at(id: &str, score: u32, observed_at_ms: u64) -> QualityObservation {
        QualityObservation {
            observed_at_ms,
            ..observation(id, score)
        }
    }

    fn window(start_ms: u64, end_ms: u64) -> DelegationQualityWindow {
        DelegationQualityWindow {
            start_ms,
            end_ms,
            observation_count: 0,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R157.2
    #[test]
    fn quality_gate_admission() {
        let knobs = DelegationKnobs {
            quality_min: 800_000,
            min_observations: 2,
            min_window_s: 60,
            window_max_age_s: 86_400,
        };
        let threshold = QualityThreshold {
            metric_name: "task_success".into(),
            minimum_value: 800_000,
            min_observations: 2,
        };
        // Equal to the threshold admits (mean of 900000 and 700000 is 800000).
        let equal = [at("o1", 900_000, 10), at("o2", 700_000, 20)];
        let admitted = admit_delegation(&window(0, 100), &equal, Some(&threshold), &knobs);
        assert!(admitted.admitted);
        assert_eq!(admitted.quality_value, 800_000);
        assert_eq!(admitted.reason, DELEGATION_ADMITTED);
        assert_eq!(admitted.quality_evidence_digest, evidence_digest(&equal));
        // One micro-unit below refuses with the stable reason.
        let below = [at("o1", 900_000, 10), at("o2", 699_998, 20)];
        let refused = admit_delegation(&window(0, 100), &below, Some(&threshold), &knobs);
        assert!(!refused.admitted);
        assert_eq!(refused.reason, "DELEGATION_BELOW_THRESHOLD");
        assert_eq!(
            refused.quality_value, 799_999,
            "the mean rounds toward zero"
        );
        // Too few observations in the window refuse; those outside it are ignored.
        let outside = [at("o1", 900_000, 10), at("o2", 900_000, 500)];
        let few = admit_delegation(&window(0, 100), &outside, Some(&threshold), &knobs);
        assert!(!few.admitted);
        assert_eq!(few.quality_evidence_digest, evidence_digest(&outside[..1]));
        // The window bounds are inclusive.
        let edges = [at("o1", 900_000, 0), at("o2", 900_000, 100)];
        assert!(admit_delegation(&window(0, 100), &edges, Some(&threshold), &knobs).admitted);
        // An absent threshold takes the knob values.
        assert!(admit_delegation(&window(0, 100), &equal, None, &knobs).admitted);
        let strict = DelegationKnobs {
            quality_min: 800_001,
            ..knobs
        };
        assert!(!admit_delegation(&window(0, 100), &equal, None, &strict).admitted);
        // No observations: quality 0, not admitted.
        let none = admit_delegation(&window(0, 100), &[], None, &knobs);
        assert!(!none.admitted);
        assert_eq!(none.quality_value, 0);
        // Window replacement: a newer or equal end replaces, an older one does not.
        assert!(replaces_window(None, &window(0, 100)));
        assert!(replaces_window(Some(&window(0, 100)), &window(50, 200)));
        assert!(replaces_window(Some(&window(0, 100)), &window(0, 100)));
        assert!(!replaces_window(Some(&window(0, 100)), &window(0, 99)));
        let resolved = DelegationKnobs::from_policy(
            &crate::decision::policy::DecisionPolicy::engine_default(),
        );
        assert_eq!(
            resolved,
            DelegationKnobs {
                quality_min: 800_000,
                min_observations: 20,
                min_window_s: 3_600,
                window_max_age_s: 86_400,
            }
        );
    }
}
