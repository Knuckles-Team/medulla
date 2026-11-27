//! Strategy specifications and proposed actions (MEDULLA-FINANCE-PRIMITIVES-R007.1).
//!
//! Evaluation is deterministic and emits PROPOSED ACTIONS ONLY: it never places or authorizes
//! an order. The types live here so the `FinanceMarket` wire carries them; the evaluator is
//! `medulla_finance::strategy::evaluate`.

use serde::{Deserialize, Serialize};

/// Schema version of [`StrategySpec`]. Bump on any breaking field change.
pub const STRATEGY_SPEC_VERSION: u32 = 1;

/// A versioned, typed strategy specification.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StrategySpec {
    pub version: u32,
    pub kind: StrategyKind,
}

/// The default strategies named by MEDULLA-FINANCE-PRIMITIVES-R007. Only the
/// dollar-cost-averaging-by-fixed-amount variant is evaluated in this slice; the
/// others are declared so the wire type is stable across later slices that add their
/// evaluators.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StrategyKind {
    /// Buy a fixed cash amount (minor units, e.g. cents) on each scheduled
    /// contribution date.
    DollarCostAverageFixedAmount { amount_minor_units: i64 },
}

/// One proposed action. Evaluation never places or authorizes an order: a
/// proposal is advisory output only, consumed by a separate order-authorization
/// boundary.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposedAction {
    pub strategy_version: u32,
    pub side: OrderSide,
    pub amount_minor_units: i64,
}

/// The side of a proposed order.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum OrderSide {
    Buy,
}
