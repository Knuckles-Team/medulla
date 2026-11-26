//! The budget a metered read runs under and the cost it reports
//! (MEDULLA-DECISION-ENGINE-R144.1, R144.5, R144.8).
//!
//! `IndexlessRetrieve` and `Uql` share one budget vocabulary: a caller asks for an
//! [`IndexlessBudget`], the engine clamps it to the tenant ceiling and answers a
//! [`CostReport`] naming the budget it applied. The types live here, the lowest crate
//! both the query wire (`UqlResult`) and the decision plane consult, so no second
//! budget or cost shape exists.

use medulla_types::contract::BoundedVec;
use serde::{Deserialize, Serialize};

/// Most operations one metered request may carry.
pub const MAX_INDEXLESS_OPS: usize = 64;

/// A budget in rows, visits and cost units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexlessBudget {
    pub max_rows: u64,
    pub max_visits: u64,
    pub max_cost_units: u64,
}

/// What a request spent and has left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CostReport {
    pub rows_visited: u64,
    pub visits: u64,
    pub cost_used: u64,
    pub remaining: IndexlessBudget,
    pub applied_budget: IndexlessBudget,
    #[serde(default)]
    pub operation_ids: BoundedVec<String, MAX_INDEXLESS_OPS>,
    pub authorization_revision: u64,
    /// The tenant ceiling the budget was clamped to, as
    /// `knob:indexless.<name>=<value>` premises (MEDULLA-DECISION-ENGINE-R144.7).
    #[serde(default)]
    pub premises: Vec<String>,
}
