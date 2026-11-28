//! The data types of the limit guard (MEDULLA-DECISION-ENGINE-R131.1, R131.3).
//!
//! They live here, below the outcome types, because [`super::policy::DecisionPolicy`]
//! holds the rules and [`super::statistical::TypedValue`] carries the action, and
//! both are in this crate. The checking behaviour is
//! `medulla_decision_outcome_types::decision::limit_guard`.

use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// The proposed action breaches a pinned limit.
pub const LIMIT_BREACH: RefusalCode = RefusalCode::new("LIMIT_BREACH");

/// The `limits` table of registered refusal codes (MEDULLA-DECISION-ENGINE-R010.100).
pub const REFUSALS: &[RefusalCode] = &[LIMIT_BREACH];

/// The param a question carries its proposed action in.
pub const GUARDED_ACTION_PARAM: &str = "guarded_action";

/// The closed metric set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LimitMetric {
    PositionSize,
    Drawdown,
    SectorConcentration,
}

impl LimitMetric {
    /// The named default check for this metric: an observed value above the
    /// bound is a breach. Every current metric is an upper bound.
    pub fn breaches(self, observed: i64, bound: i64) -> bool {
        match self {
            Self::PositionSize | Self::Drawdown | Self::SectorConcentration => observed > bound,
        }
    }
}

/// One configured limit. Values are integer units of the metric (for example
/// minor currency units or basis points).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LimitRule {
    pub id: String,
    pub metric: LimitMetric,
    pub bound: i64,
    pub scope: String,
}

/// One observed metric value of the proposed action, in one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LimitObservation {
    pub metric: LimitMetric,
    pub scope: String,
    pub value: i64,
}

/// A request-supplied bound for one rule. It can only tighten that rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RequestedBound {
    pub rule_id: String,
    pub bound: i64,
}

/// The proposed action as the guard sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GuardedAction {
    pub signal_iri: String,
    pub observations: Vec<LimitObservation>,
    #[serde(default)]
    pub requested_bounds: Vec<RequestedBound>,
}
