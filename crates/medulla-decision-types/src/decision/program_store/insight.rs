//! Run-derived insights keyed by context signature: the model
//! (MEDULLA-DECISION-ENGINE-R135.10).
//!
//! An insight is a hint distilled from successful runs. It names the runs it
//! came from, the context signature it applies to, how many times it has been
//! confirmed and the revision of the selection that last chose it. Memory notes
//! stay in R141.100. Nothing here reads a clock or a store.

use super::super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::super::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// Most source runs one insight may cite. A `Fixed` safety bound.
pub const MAX_INSIGHT_SOURCE_RUNS: usize = 64;
/// Current wire schema version of [`Insight`].
pub const INSIGHT_SCHEMA_VERSION: u32 = 1;

/// Fewest insights one selection may return.
pub const MIN_SELECTED_INSIGHTS: i64 = 1;
/// Most insights one selection may return. A `Fixed` safety bound.
pub const MAX_SELECTED_INSIGHTS: i64 = 32;

/// The `insight.*` knobs (MEDULLA-DECISION-ENGINE-R135.11): the lowest context
/// match an insight needs (a fraction, in millionths) and the most insights one
/// selection returns.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "insight.min_match",
        unit: "fraction",
        default: KnobValue::DecimalMicros(300_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "insight.max_selected",
        unit: "count",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(MIN_SELECTED_INSIGHTS),
        max: KnobValue::Integer(MAX_SELECTED_INSIGHTS),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved values of the `insight.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsightKnobs {
    /// The lowest Jaccard match, in millionths.
    pub min_match_micros: i64,
    pub max_selected: usize,
}

impl InsightKnobs {
    /// The values `policy` resolves, clamped to the declared ranges.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let integer = |value: KnobValue| match value {
            KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
            KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
        };
        let min_match_micros = integer(policy.knob(&KNOBS[0])).clamp(0, 1_000_000);
        let max_selected =
            integer(policy.knob(&KNOBS[1])).clamp(MIN_SELECTED_INSIGHTS, MAX_SELECTED_INSIGHTS);
        Self {
            min_match_micros,
            max_selected: usize::try_from(max_selected).unwrap_or(1),
        }
    }
}

/// Refusal: an insight that cites no source run.
pub const MISSING_SOURCE_RUN: RefusalCode = RefusalCode::new("MISSING_SOURCE_RUN");

/// Abstain reason: the context signature is empty or shares no token with any
/// stored insight (MEDULLA-DECISION-ENGINE-R135.13).
pub const UNKNOWN_CONTEXT_SIGNATURE: RefusalCode = RefusalCode::new("UNKNOWN_CONTEXT_SIGNATURE");

/// Every refusal code of the insight-selection family; joins
/// `codes::FAMILY_TABLES` under `QuestionKind::InsightSelection`.
pub const REFUSALS: &[RefusalCode] = &[MISSING_SOURCE_RUN, UNKNOWN_CONTEXT_SIGNATURE];

/// The decision-artifact prefix every stored insight lives under.
pub const INSIGHT_KEY_PREFIX: &str = "run_record/insight/";

/// The decision-artifact key of `insight`:
/// `run_record/insight/<context_signature_digest>/<insight_id>`.
pub fn insight_key(insight: &Insight) -> String {
    use sha2::{Digest, Sha256};
    let digest: String = Sha256::digest(insight.context_signature.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{INSIGHT_KEY_PREFIX}{digest}/{}", insight.insight_id)
}

/// A selection that matched no stored insight: the abstain reason
/// `UNKNOWN_CONTEXT_SIGNATURE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownContextSignature {
    pub context_signature: String,
}

impl crate::decision::record::outcome::DecisionRefusal for UnknownContextSignature {
    fn code(&self) -> RefusalCode {
        UNKNOWN_CONTEXT_SIGNATURE
    }

    fn detail(&self) -> String {
        format!(
            "no stored insight applies to context_signature `{}`",
            self.context_signature
        )
    }
}

/// One run-derived hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Insight {
    pub schema_version: u32,
    pub insight_id: String,
    pub text: String,
    /// The runs the insight was derived from; at least one.
    pub source_run_ids: BoundedVec<String, MAX_INSIGHT_SOURCE_RUNS>,
    pub context_signature: String,
    pub success_count: u32,
    pub selection_revision: u64,
}

/// Why an insight was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsightError {
    /// No source run (`MISSING_SOURCE_RUN`).
    MissingSourceRun,
    /// The wire value did not decode.
    Malformed(String),
    /// A field broke an invariant.
    Invalid(String),
}

impl InsightError {
    /// The registered refusal code, when the error is one.
    pub fn code(&self) -> Option<RefusalCode> {
        matches!(self, Self::MissingSourceRun).then_some(MISSING_SOURCE_RUN)
    }
}

impl std::fmt::Display for InsightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSourceRun => write!(
                f,
                "{}: an insight cites at least one source run",
                MISSING_SOURCE_RUN.as_str()
            ),
            Self::Malformed(detail) => write!(f, "insight is malformed: {detail}"),
            Self::Invalid(detail) => write!(f, "insight is invalid: {detail}"),
        }
    }
}

impl std::error::Error for InsightError {}

impl Insight {
    /// Decode a wire value. An insight with no source run is
    /// `MISSING_SOURCE_RUN`, whatever else is wrong with it.
    pub fn from_value(value: serde_json::Value) -> Result<Self, InsightError> {
        let empty = match value.get("source_run_ids") {
            None => true,
            Some(ids) => ids.as_array().is_some_and(|ids| ids.is_empty()),
        };
        if empty {
            return Err(InsightError::MissingSourceRun);
        }
        let insight: Self = serde_json::from_value(value)
            .map_err(|error| InsightError::Malformed(error.to_string()))?;
        insight.validate()?;
        Ok(insight)
    }

    /// Check every invariant: schema version, at least one non-blank source
    /// run, non-blank id, text and context signature.
    pub fn validate(&self) -> Result<(), InsightError> {
        let invalid = |detail: &str| Err(InsightError::Invalid(detail.to_string()));
        if self.schema_version != INSIGHT_SCHEMA_VERSION {
            return invalid("unsupported schema_version");
        }
        if self.source_run_ids.is_empty() {
            return Err(InsightError::MissingSourceRun);
        }
        if self.source_run_ids.iter().any(|id| id.trim().is_empty()) {
            return invalid("source run ids are non-empty");
        }
        if self.insight_id.trim().is_empty()
            || self.text.trim().is_empty()
            || self.context_signature.trim().is_empty()
        {
            return invalid("insight_id, text and context_signature are required");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insight(runs: &[&str]) -> Insight {
        Insight {
            schema_version: INSIGHT_SCHEMA_VERSION,
            insight_id: "ins-1".into(),
            text: "retry the export once before escalating".into(),
            source_run_ids: BoundedVec::new(runs.iter().map(|run| run.to_string()).collect())
                .unwrap(),
            context_signature: "export-report".into(),
            success_count: 3,
            selection_revision: 7,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.10
    #[test]
    fn insight_without_source_rejected() {
        let original = insight(&["run-1", "run-2"]);
        original.validate().unwrap();

        // Every field survives JSON and MessagePack.
        let wire = serde_json::to_value(&original).unwrap();
        assert_eq!(Insight::from_value(wire.clone()).unwrap(), original);
        let packed = rmp_serde::to_vec_named(&original).unwrap();
        let unpacked: Insight = rmp_serde::from_slice(&packed).unwrap();
        assert_eq!(unpacked, original);
        assert_eq!(
            serde_json::to_string(&crate::decision::statistical::QuestionKind::InsightSelection)
                .unwrap(),
            "\"insight_selection\""
        );

        // An empty source list is MISSING_SOURCE_RUN, on the wire and in memory.
        let mut empty = wire.clone();
        empty["source_run_ids"] = serde_json::json!([]);
        let refused = Insight::from_value(empty).unwrap_err();
        assert_eq!(refused, InsightError::MissingSourceRun);
        assert_eq!(refused.code(), Some(MISSING_SOURCE_RUN));
        assert!(refused.to_string().starts_with("MISSING_SOURCE_RUN"));
        let none = insight(&[]);
        assert_eq!(none.validate(), Err(InsightError::MissingSourceRun));
        let mut absent = wire.clone();
        absent.as_object_mut().unwrap().remove("source_run_ids");
        assert_eq!(
            Insight::from_value(absent),
            Err(InsightError::MissingSourceRun)
        );

        // A blank source id, an unknown field and a 65th source run are refused.
        assert!(matches!(
            insight(&[" "]).validate(),
            Err(InsightError::Invalid(_))
        ));
        let mut unknown = wire.clone();
        unknown["extra"] = serde_json::json!(1);
        assert!(matches!(
            Insight::from_value(unknown),
            Err(InsightError::Malformed(_))
        ));
        let mut many = wire.clone();
        many["source_run_ids"] = serde_json::json!((0..=MAX_INSIGHT_SOURCE_RUNS)
            .map(|index| format!("run-{index}"))
            .collect::<Vec<_>>());
        assert!(matches!(
            Insight::from_value(many),
            Err(InsightError::Malformed(_))
        ));
        let full = (0..MAX_INSIGHT_SOURCE_RUNS)
            .map(|index| format!("run-{index}"))
            .collect::<Vec<_>>();
        let refs: Vec<&str> = full.iter().map(String::as_str).collect();
        insight(&refs).validate().unwrap();
    }
}
