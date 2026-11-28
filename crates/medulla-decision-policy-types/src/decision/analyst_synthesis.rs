//! Cited analyst synthesis with required abstention
//! (MEDULLA-DECISION-ENGINE-R134.13).
//!
//! `Decide` with
//! [`QuestionKind::AnalystSynthesis`](super::statistical::QuestionKind::AnalystSynthesis)
//! accounts for every expected analyst, finds conflicting claims and checks
//! every citation against the caller's visible graph records. Any failure
//! abstains with no recommendation. Nothing here reads a clock or a store.

use serde::{Deserialize, Serialize};

use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_decision_statistical_types::decision::record::outcome::refusal::DecisionRefusal;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// A conflict: two findings share a claim key and differ in claim digest.
pub const UNRESOLVED_CONFLICT: &str = "UNRESOLVED_CONFLICT";
/// A finding has fewer citations than `synthesis.min_citations`.
pub const MISSING_CITATION: &str = "MISSING_CITATION";
/// A citation names no visible record, or a record whose digest differs.
pub const INVALID_CITATION: &str = "INVALID_CITATION";
/// An expected analyst has neither a finding nor a gap.
pub const ANALYST_MISSING: &str = "ANALYST_MISSING";
/// An expected analyst appears more than once.
pub const ANALYST_DUPLICATE: &str = "ANALYST_DUPLICATE";
/// The decision role is not among the analysts.
pub const DECISION_ROLE_ABSENT: &str = "DECISION_ROLE_ABSENT";
/// The request's plan digest is not the plan the analysts ran.
pub const SYNTHESIS_PLAN_MISMATCH: &str = "SYNTHESIS_PLAN_MISMATCH";

/// Every refusal code of the synthesis family; joins `code_registry::FAMILY_TABLES`
/// under `QuestionKind::AnalystSynthesis`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(UNRESOLVED_CONFLICT),
    RefusalCode::new(MISSING_CITATION),
    RefusalCode::new(INVALID_CITATION),
    RefusalCode::new(ANALYST_MISSING),
    RefusalCode::new(ANALYST_DUPLICATE),
    RefusalCode::new(DECISION_ROLE_ABSENT),
    RefusalCode::new(SYNTHESIS_PLAN_MISMATCH),
];

/// The `synthesis.*` knob: fewest citations one finding carries, in citations.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "synthesis.min_citations",
    unit: "count",
    default: KnobValue::Integer(1),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(10),
    safety: KnobSafety::Tunable,
}];

/// The resolved `synthesis.min_citations`, held inside `[1, 10]`.
pub fn min_citations(policy: &DecisionPolicy) -> usize {
    let value = match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
        KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
    };
    usize::try_from(value.clamp(1, 10)).unwrap_or(1)
}

/// One cited piece of evidence of a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Citation {
    pub evidence_id: String,
    pub revision: u64,
    pub digest: String,
    pub span: String,
}

/// One analyst's finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AnalystFinding {
    pub finding_id: String,
    pub analyst_id: String,
    pub role: String,
    pub claim_key: String,
    pub claim_digest: String,
    pub citations: Vec<Citation>,
}

/// An analyst that failed; it stays in the accounting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AnalystGap {
    pub analyst_id: String,
    pub reason: String,
}

/// The request of an `analyst_synthesis` question, carried as the JSON
/// `synthesis` parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SynthesisRequest {
    pub plan_digest: String,
    pub expected_analyst_ids: Vec<String>,
    pub decision_role_id: String,
    pub findings: Vec<AnalystFinding>,
    pub gaps: Vec<AnalystGap>,
}

/// How one expected analyst is accounted for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AnalystAccount {
    Findings {
        analyst_id: String,
        finding_ids: Vec<String>,
    },
    Gap {
        analyst_id: String,
        reason: String,
    },
}

/// One consistent claim and the findings that support it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SupportedConclusion {
    pub claim_key: String,
    pub claim_digest: String,
    pub finding_ids: Vec<String>,
}

/// The synthesis the decision answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SynthesisResult {
    pub decision_role_id: String,
    pub conclusions: Vec<SupportedConclusion>,
    pub accounting: Vec<AnalystAccount>,
}

/// Why a synthesis abstains: one registered code and the detail naming the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl SynthesisRefusal {
    /// A refusal under `code`, which must be one of [`REFUSALS`].
    pub fn new(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            code: RefusalCode::new(code),
            detail: detail.to_string(),
        }
    }
}

impl DecisionRefusal for SynthesisRefusal {
    fn code(&self) -> RefusalCode {
        self.code
    }

    fn detail(&self) -> String {
        self.detail.clone()
    }
}
