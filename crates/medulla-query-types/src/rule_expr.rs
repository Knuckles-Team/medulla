//! The wire form of the UQL `EVALUATE RULE` stage (MEDULLA-BUSINESS-OPS-R002.7).

use serde::{Deserialize, Serialize};

/// A UQL `EVALUATE RULE <id> [ASOF <int>] [COUNT <int>] [TEXT <string> [LOCALE <string>]]`
/// stage: which stored `:RuleSpec` node evaluates, at which explicit as-of time (epoch
/// milliseconds), for how many due dates (a recurrence rule) and over which text (a
/// phrase policy). The stage never reads a clock: a missing `as_of_ms` is refused by the
/// executor with `ASOF_REQUIRED`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvaluateRuleOp {
    /// The node id of the rule record.
    pub rule: String,
    /// The as-of time in epoch milliseconds.
    #[serde(default)]
    pub as_of_ms: Option<i64>,
    /// How many due dates a recurrence rule returns.
    #[serde(default)]
    pub count: Option<u32>,
    /// The text a phrase policy checks.
    #[serde(default)]
    pub text: Option<String>,
    /// The locale of `text`.
    #[serde(default)]
    pub locale: Option<String>,
}
