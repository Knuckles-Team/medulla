//! Data quality rules and scores (MEDULLA-UNIFIED-DATA-PLANE-R762.1). A rule
//! targets one dataset or column in one closed quality dimension; a score
//! records one evaluation of it, native or from an outside checker.

use serde::{Deserialize, Serialize};

/// The closed set of quality dimensions shared by native and outside scores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QualityDimension {
    Completeness,
    Uniqueness,
    Validity,
    Consistency,
    Freshness,
    Accuracy,
}

/// How severe a failing rule is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QualitySeverity {
    Info,
    Warning,
    Critical,
}

/// One quality rule. `threshold` is the minimum passing value in `[0, 1]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityRule {
    pub id: String,
    pub target: String,
    pub dimension: QualityDimension,
    pub expression: String,
    pub threshold: f64,
    pub severity: QualitySeverity,
}

/// The `checker` of a score the engine measured itself.
pub const NATIVE_CHECKER: &str = "native";

/// The evidence one score records (MEDULLA-UNIFIED-DATA-PLANE-R762.5): the compiled
/// statement or shape it ran, the rows it saw and the checker that measured it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreEvidence {
    /// SHA-256 over the compiled statement or shape IRI, 64 lowercase hex characters.
    pub query_digest: String,
    pub rows_checked: u64,
    /// At most `rows_checked`.
    pub rows_failed: u64,
    /// [`NATIVE_CHECKER`] for engine scores.
    pub checker: String,
}

/// One evaluation of a rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityScore {
    pub rule: String,
    pub value: f64,
    pub passed: bool,
    pub at: i64,
    /// SHA-256 over the compiled statement or shape IRI, 64 lowercase hex characters.
    pub query_digest: String,
    pub rows_checked: u64,
    /// At most `rows_checked`.
    pub rows_failed: u64,
    /// [`NATIVE_CHECKER`] for engine scores.
    pub checker: String,
}

/// A rule failed validation, with its stable refusal code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidQualityRule {
    /// `QUALITY_RULE_INVALID`: no id, target or expression.
    RuleInvalid(String),
    /// `QUALITY_THRESHOLD_INVALID`: threshold outside `[0, 1]`.
    ThresholdInvalid,
}

impl InvalidQualityRule {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RuleInvalid(_) => "QUALITY_RULE_INVALID",
            Self::ThresholdInvalid => "QUALITY_THRESHOLD_INVALID",
        }
    }
}

impl std::fmt::Display for InvalidQualityRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuleInvalid(reason) => write!(f, "{}: {reason}", self.code()),
            Self::ThresholdInvalid => write!(f, "{}: threshold outside 0 to 1", self.code()),
        }
    }
}

impl std::error::Error for InvalidQualityRule {}

impl QualityRule {
    /// Refuse a rule with a blank id, target or expression, or a threshold
    /// outside `[0, 1]`.
    pub fn validate(&self) -> Result<(), InvalidQualityRule> {
        for (name, value) in [
            ("id", &self.id),
            ("target", &self.target),
            ("expression", &self.expression),
        ] {
            if value.trim().is_empty() {
                return Err(InvalidQualityRule::RuleInvalid(format!("blank {name}")));
            }
        }
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(InvalidQualityRule::ThresholdInvalid);
        }
        Ok(())
    }

    /// Score one measured value against this rule, recording its evidence.
    pub fn score(&self, value: f64, at: i64, evidence: ScoreEvidence) -> QualityScore {
        QualityScore {
            rule: self.id.clone(),
            value,
            passed: value >= self.threshold,
            at,
            query_digest: evidence.query_digest,
            rows_checked: evidence.rows_checked,
            rows_failed: evidence.rows_failed.min(evidence.rows_checked),
            checker: evidence.checker,
        }
    }
}

/// Where a proposed rule stands in the change-request flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RuleState {
    Proposed,
    Approved { approver: String },
}

impl RuleState {
    /// The machine kind of this state (MEDULLA-DATAPLAT-VOCAB-R009.6). The approver a state
    /// carries stays on the record.
    pub const fn kind(&self) -> medulla_types::quality_rule_machine::RuleStateKind {
        use medulla_types::quality_rule_machine::RuleStateKind as K;
        match self {
            Self::Proposed => K::Proposed,
            Self::Approved { .. } => K::Approved,
        }
    }

    /// The state a new rule takes: the initial state of `QualityRuleMachine`.
    pub fn initial() -> Self {
        use medulla_types::quality_rule_machine::RuleStateKind as K;
        match K::initial() {
            K::Proposed => Self::Proposed,
            K::Approved => Self::Approved {
                approver: String::new(),
            },
        }
    }

    /// Approve the rule through `QualityRuleMachine`: the approved state, or
    /// `TRANSITION_REFUSED` when the rule is not proposed.
    pub fn approve(
        &self,
        approver: impl Into<String>,
    ) -> Result<Self, medulla_types::transition_refused::TransitionRefused> {
        use medulla_types::quality_rule_machine::{QualityRuleEvent, RuleStateKind as K};
        self.kind()
            .apply(QualityRuleEvent::Approve)
            .map(|kind| match kind {
                K::Approved => Self::Approved {
                    approver: approver.into(),
                },
                K::Proposed => Self::Proposed,
            })
    }
}

/// What one `QualityRulePut` wrote (MEDULLA-UNIFIED-DATA-PLANE-R762.3): the rule, the
/// tenant-private node that holds it and the state it entered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityRuleReceipt {
    pub rule_id: String,
    pub node_id: String,
    pub state: RuleState,
}

/// A rule proposed from a column profile (MEDULLA-UNIFIED-DATA-PLANE-R762.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposedQualityRule {
    pub rule: QualityRule,
    pub state: RuleState,
    /// Why the profile suggested this rule.
    pub basis: String,
}

impl ProposedQualityRule {
    /// The rule to evaluate, only once approved. A proposed rule never
    /// evaluates as active.
    pub fn active(&self) -> Option<&QualityRule> {
        use medulla_types::quality_rule_machine::RuleStateKind as K;
        match self.state.kind() {
            K::Approved => Some(&self.rule),
            K::Proposed => None,
        }
    }

    /// Record the approval; a blank approver changes nothing.
    pub fn approve(&mut self, approver: &str) -> bool {
        if approver.trim().is_empty() {
            return false;
        }
        match self.state.approve(approver) {
            Ok(next) => {
                self.state = next;
                true
            }
            Err(_) => false,
        }
    }
}

fn digit_pattern(value: &str) -> Option<usize> {
    (!value.is_empty() && value.chars().all(|c| c.is_ascii_digit())).then_some(value.len())
}

/// Propose candidate rules from one profile: a low null ratio yields a
/// completeness rule, a key-like distinct count a uniqueness rule, and a
/// fixed-width digit pattern a validity rule. Every candidate is `Proposed`.
pub fn propose_rules(profile: &crate::profile::ColumnProfile) -> Vec<ProposedQualityRule> {
    let target = format!("{}.{}", profile.dataset, profile.column);
    let candidate = |suffix: &str, dimension, expression: String, threshold, basis: String| {
        ProposedQualityRule {
            rule: QualityRule {
                id: format!("{target}.{suffix}"),
                target: target.clone(),
                dimension,
                expression,
                threshold,
                severity: QualitySeverity::Warning,
            },
            state: RuleState::initial(),
            basis,
        }
    };
    let mut out = Vec::new();
    let null_ratio = profile.null_ratio();
    if profile.rows > 0 && null_ratio < 0.05 {
        out.push(candidate(
            "completeness",
            QualityDimension::Completeness,
            format!("count({}) / count(*)", profile.column),
            ((1.0 - null_ratio) * 100.0).floor() / 100.0,
            format!("null ratio {null_ratio:.4}"),
        ));
    }
    let non_null = profile.rows - profile.nulls;
    if non_null > 0 && profile.distinct_estimate >= non_null {
        out.push(candidate(
            "uniqueness",
            QualityDimension::Uniqueness,
            format!("count(distinct {0}) / count({0})", profile.column),
            1.0,
            "distinct count equals non-null rows".to_string(),
        ));
    }
    let widths = (
        profile.min.as_deref().and_then(digit_pattern),
        profile.max.as_deref().and_then(digit_pattern),
    );
    if let (Some(a), Some(b)) = widths {
        if a == b {
            out.push(candidate(
                "validity",
                QualityDimension::Validity,
                format!("{} ~ '^[0-9]{{{a}}}$'", profile.column),
                1.0,
                format!("min and max are {a}-digit values"),
            ));
        }
    }
    out
}
