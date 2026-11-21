//! Limit guard over a proposed action (MEDULLA-DECISION-ENGINE-R131.1 and R131.2).
//!
//! Each rule names one closed metric, a bound and the scope it covers. A
//! request may supply its own bounds, but only a tighter bound takes effect.

use medulla_decision_types::decision::limit_types::{GuardedAction, LimitRule};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LimitBreach {
    pub rule_id: String,
    pub observed: i64,
    pub bound: i64,
    pub signal_iri: String,
}

fn effective_bound(rule: &LimitRule, action: &GuardedAction) -> i64 {
    action
        .requested_bounds
        .iter()
        .filter(|requested| requested.rule_id == rule.id)
        .map(|requested| requested.bound)
        .fold(rule.bound, i64::min)
}

/// Every breach of `rules` by `action`, in rule order.
pub fn check_limits(action: &GuardedAction, rules: &[LimitRule]) -> Vec<LimitBreach> {
    rules
        .iter()
        .filter_map(|rule| {
            let bound = effective_bound(rule, action);
            action
                .observations
                .iter()
                .filter(|o| o.metric == rule.metric && o.scope == rule.scope)
                .map(|o| o.value)
                .max()
                .filter(|observed| rule.metric.breaches(*observed, bound))
                .map(|observed| LimitBreach {
                    rule_id: rule.id.clone(),
                    observed,
                    bound,
                    signal_iri: action.signal_iri.clone(),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests;
