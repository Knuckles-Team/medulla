//! The served accounting projection result (MEDULLA-FINANCE-PRIMITIVES-R013.2.6).
//!
//! `Method::FinanceAccountProject` answers one [`FinanceAccountProjection`]: a
//! discardable read model of one account at one `(as_of, known_at)` point.
//! Every figure is fixed-point ticks (never `f64`) and carries its source,
//! as-of time and session; a figure without all three is never returned.

use serde::{Deserialize, Serialize};

/// The refusal code of a figure whose source, as-of or session is missing.
pub const FIGURE_PROVENANCE_MISSING: &str = "FIGURE_PROVENANCE_MISSING";
/// The refusal code of a basis policy the projection does not apply.
pub const BASIS_POLICY_UNSUPPORTED: &str = "BASIS_POLICY_UNSUPPORTED";

/// The closed set of figure names a projection returns, per position.
pub const FIGURE_NAMES: [&str; 5] = [
    "open_quantity",
    "cost_basis",
    "realized_gain",
    "market_value",
    "dividend_cash",
];

/// One displayed figure with its full provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectedFigure {
    /// The position (instrument) the figure belongs to.
    pub position: String,
    /// One of [`FIGURE_NAMES`].
    pub name: String,
    /// Fixed-point value in ticks at [`FINANCE_SCALE`].
    pub value_ticks: i64,
    pub source: String,
    pub as_of: String,
    pub session: String,
}

/// One whole unit of quantity or currency is this many ticks.
pub const FINANCE_SCALE: i64 = 100_000_000;

/// The projection of one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinanceAccountProjection {
    pub account: String,
    pub as_of: String,
    pub known_at: String,
    pub basis_policy: String,
    pub base_currency: String,
    pub scale: i64,
    /// SHA-256 over the ordered input activity terms and the policy, so the
    /// read model is keyed by its inputs.
    pub input_digest: String,
    pub figures: Vec<ProjectedFigure>,
}

/// The refusal code of a range the performance method does not know.
pub const RANGE_UNSUPPORTED: &str = "RANGE_UNSUPPORTED";
/// The refusal code of an account whose binding to the verified principal is not recorded.
pub const ACCOUNT_BINDING_UNVERIFIED: &str = "ACCOUNT_BINDING_UNVERIFIED";
/// The refusal code of a benchmark with no recorded level.
pub const BENCHMARK_UNKNOWN: &str = "BENCHMARK_UNKNOWN";

/// The benchmark's figure over the same window as the account's
/// (MEDULLA-FINANCE-PRIMITIVES-R005.3.4). `value_ticks` and `value` are null together, then
/// `reason` says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BenchmarkFigure {
    /// The benchmark name.
    pub name: String,
    /// The return in ticks at [`FINANCE_SCALE`]; null when unavailable.
    pub value_ticks: Option<i64>,
    /// The decimal string of `value_ticks`.
    pub value: Option<String>,
    /// Why the figure is unavailable.
    pub reason: Option<String>,
}

/// One account's return over a range (MEDULLA-FINANCE-PRIMITIVES-R005.3.4). A missing figure is
/// null with a `reason`, never zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinancePerformanceCard {
    /// The account.
    pub account: String,
    /// `twr`, `mwr` or `xirr`.
    pub method: String,
    /// The base currency of every value.
    pub currency: String,
    /// The requested range.
    pub range: String,
    /// The request's as-of instant.
    pub as_of: String,
    /// The request's known-at instant.
    pub known_at: String,
    /// The return in ticks at `scale`; null when unavailable.
    pub value_ticks: Option<i64>,
    /// The decimal string of `value_ticks`, the wire form of money.
    pub value: Option<String>,
    /// Why the figure is unavailable: `NO_VALUATION`, `NO_VALUATION_AT_START`,
    /// `NON_POSITIVE_START_VALUE` or `NO_SOLUTION`.
    pub reason: Option<String>,
    /// The benchmark over the same window, when one was asked for.
    pub benchmark: Option<BenchmarkFigure>,
    /// The `recordSource` of the end valuation record.
    pub source: String,
    /// The `activitySession` of the end valuation record.
    pub session: String,
    /// True when the end valuation is more than five days before `as_of`.
    pub stale: bool,
    /// One whole unit is this many ticks.
    pub scale: i64,
    /// SHA-256 over the request and the ordered records used.
    pub input_digest: String,
}
