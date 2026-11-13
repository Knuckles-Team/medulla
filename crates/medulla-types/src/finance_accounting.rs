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
