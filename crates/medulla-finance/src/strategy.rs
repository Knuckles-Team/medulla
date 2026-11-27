// CONCEPT:MEDULLA-KG.domains.finance-compute.strategy — Strategy Evaluation Engine
//
// Versioned StrategySpec evaluation (MEDULLA-FINANCE-PRIMITIVES-R007). Evaluation is
// deterministic and emits PROPOSED ACTIONS ONLY: it never places or authorizes
// an order. This slice (R007.1) ships the dollar-cost-averaging-by-fixed-amount
// default strategy; trend-following and calendar/threshold rebalancing are later
// slices (R007.2+).

use medulla_result_types::compute_result::market::strategy::{
    OrderSide, ProposedAction, StrategyKind, StrategySpec,
};

/// Evaluate a [`StrategySpec`] for one scheduled contribution date, deterministically.
///
/// Returns `None` when the spec proposes no action (a non-positive configured
/// amount is refused rather than silently clamped).
pub fn evaluate(spec: &StrategySpec) -> Option<ProposedAction> {
    match spec.kind {
        StrategyKind::DollarCostAverageFixedAmount { amount_minor_units } => {
            if amount_minor_units <= 0 {
                return None;
            }
            Some(ProposedAction {
                strategy_version: spec.version,
                side: OrderSide::Buy,
                amount_minor_units,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::compute_result::market::strategy::STRATEGY_SPEC_VERSION;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R007.1
    #[test]
    fn dca_fixed_amount_proposes_buy_of_configured_amount() {
        let spec = StrategySpec {
            version: STRATEGY_SPEC_VERSION,
            kind: StrategyKind::DollarCostAverageFixedAmount {
                amount_minor_units: 10_000,
            },
        };
        let action = evaluate(&spec).expect("fixed-amount DCA always proposes a buy");
        assert_eq!(action.strategy_version, STRATEGY_SPEC_VERSION);
        assert_eq!(action.side, OrderSide::Buy);
        assert_eq!(action.amount_minor_units, 10_000);
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R007.1
    #[test]
    fn dca_fixed_amount_refuses_a_non_positive_contribution() {
        let spec = StrategySpec {
            version: STRATEGY_SPEC_VERSION,
            kind: StrategyKind::DollarCostAverageFixedAmount {
                amount_minor_units: 0,
            },
        };
        assert!(evaluate(&spec).is_none());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R007.1
    #[test]
    fn evaluation_is_deterministic_across_repeated_calls() {
        let spec = StrategySpec {
            version: STRATEGY_SPEC_VERSION,
            kind: StrategyKind::DollarCostAverageFixedAmount {
                amount_minor_units: 5_000,
            },
        };
        assert_eq!(evaluate(&spec), evaluate(&spec));
    }
}
