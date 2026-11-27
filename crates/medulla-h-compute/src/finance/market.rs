//! `Method::FinanceMarket`: market bars and trend signals (EH-413..EH-418).
//!
//! Pure compute over what the request carries — no store, no clock, no order
//! authority. Inputs are bounded by the transport's request-size limit (and a
//! replay by its own work budget); every op answers its declared result or a
//! typed refusal rendered `"CODE: detail"`.

use medulla_finance_market::{
    market::backtest_run, market::codec, market::confidence, market::corporate_actions,
    market::decimate, market::indicators, market::liquidity, market::rollup, market::scan,
    market::signal, market::snapshot, market::MarketError, market::MarketResult,
    market::INVALID_REQUEST,
};
use medulla_protocol_types::result_contract::compute as results;
use medulla_protocol_types::result_contract::MethodResult;
use medulla_result_types::compute_result::market::FinanceMarketOp;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;

pub(super) fn answer<M: MethodResult>(req_id: u64, result: MarketResult<M::Body>) -> Response {
    match result {
        Ok(body) => Response::ok(req_id, ResultPayload::of::<M>(body)),
        // The market domain codes (`UNSOURCED_CLAIM`, ...) are not in the
        // FinanceMarket error contract; keep them first in the detail.
        Err(error) => Response::err(req_id, format!("INVALID_ARGUMENT: {error}")),
    }
}

/// Serve one `FinanceMarket` op.
pub fn handle(req_id: u64, op: FinanceMarketOp) -> Response {
    match op {
        FinanceMarketOp::EncodePoints { records } => {
            answer::<results::FinanceMarketEncodePoints>(req_id, codec::encode_all(&records))
        }
        FinanceMarketOp::Resolve {
            records,
            points,
            as_of,
            finality,
        } => answer::<results::FinanceMarketResolve>(
            req_id,
            codec::resolve_with_points(records, &points, as_of, finality),
        ),
        FinanceMarketOp::Rollup {
            bars,
            calendar,
            timeframe,
            watermark,
        } => answer::<results::FinanceMarketRollup>(
            req_id,
            rollup::rollup(&bars, &calendar, timeframe, watermark),
        ),
        FinanceMarketOp::Indicators { bars, spec } => {
            answer::<results::FinanceMarketIndicators>(req_id, indicators::compute(&bars, &spec))
        }
        FinanceMarketOp::SignalReplay { request } => {
            answer::<results::FinanceMarketSignalReplay>(req_id, signal::replay(&request))
        }
        FinanceMarketOp::SignalAdvance { checkpoint, bars } => {
            answer::<results::FinanceMarketSignalAdvance>(
                req_id,
                signal::advance_wire(&checkpoint, &bars),
            )
        }
        FinanceMarketOp::SignalScan { request } => {
            answer::<results::FinanceMarketSignalScan>(req_id, scan::scan(&request))
        }
        FinanceMarketOp::FlipConfidence { request } => {
            answer::<results::FinanceMarketFlipConfidence>(
                req_id,
                confidence::flip_confidence(&request),
            )
        }
        FinanceMarketOp::BacktestRun { draft } => {
            answer::<results::FinanceMarketBacktestRun>(req_id, backtest_run::seal(&draft))
        }
        FinanceMarketOp::Decimate { request } => {
            answer::<results::FinanceMarketDecimate>(req_id, decimate::decimate(&request))
        }
        FinanceMarketOp::LiquidityPanel { request } => {
            answer::<results::FinanceMarketLiquidityPanel>(req_id, liquidity::compute(&request))
        }
        FinanceMarketOp::AnalysisSnapshot { draft } => {
            answer::<results::FinanceMarketAnalysisSnapshot>(req_id, snapshot::seal(&draft))
        }
        FinanceMarketOp::CorporateActions { records, as_of } => {
            answer::<results::FinanceMarketCorporateActions>(
                req_id,
                corporate_actions::resolve(&records, as_of),
            )
        }
        FinanceMarketOp::StrategyEvaluate { spec } => {
            answer::<results::FinanceMarketStrategyEvaluate>(
                req_id,
                medulla_finance::strategy::evaluate(&spec).ok_or_else(|| {
                    MarketError::new(INVALID_REQUEST, "the strategy spec proposes no action")
                }),
            )
        }
    }
}

/// The route-family entry: a `FinanceMarket` request, or the method handed back.
pub(super) fn handle_market(req_id: u64, method: Method) -> Result<Response, Method> {
    match method {
        Method::FinanceMarket { op } => Ok(handle(req_id, *op)),
        other => Err(other),
    }
}

#[cfg(test)]
mod served_ops_tests {
    use super::*;
    use medulla_result_types::compute_result::market::{
        CorporateAction, CorporateActionKind, OrderSide, ProposedAction, StrategyKind,
        StrategySpec, STRATEGY_SPEC_VERSION,
    };

    const DAY: i64 = 86_400_000_000_000;

    fn decoded<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, String> {
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        match response.result {
            Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
            other => panic!("unexpected payload {other:?}"),
        }
    }

    fn dividend(listing: &str, effective: i64, revision: u32, known_at: i64) -> CorporateAction {
        CorporateAction {
            listing_id: listing.to_string(),
            action: CorporateActionKind::Dividend { amount_ticks: 50 },
            effective_time: effective,
            known_at,
            source: "fixture".to_string(),
            revision,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R006, MEDULLA-FINANCE-PRIMITIVES-R006.1
    #[test]
    fn the_corporate_actions_op_serves_the_as_of_view_and_refuses_a_conflict() {
        let original = dividend("L1", DAY, 0, DAY);
        let corrected = dividend("L1", DAY, 1, 5 * DAY);
        let other = dividend("L2", 2 * DAY, 0, 2 * DAY);
        let records = vec![corrected.clone(), original.clone(), other.clone()];
        let ask = |as_of: Option<i64>, records: Vec<CorporateAction>| {
            decoded::<Vec<CorporateAction>>(handle(
                1,
                FinanceMarketOp::CorporateActions { records, as_of },
            ))
        };
        assert_eq!(
            ask(Some(3 * DAY), records.clone()).unwrap(),
            vec![original.clone(), other.clone()]
        );
        assert_eq!(ask(None, records.clone()).unwrap(), vec![corrected, other]);
        assert!(ask(Some(DAY - 1), records).unwrap().is_empty());
        let mut rewrite = original.clone();
        rewrite.action = CorporateActionKind::Delisting;
        let refused = ask(None, vec![original, rewrite]).unwrap_err();
        assert!(refused.starts_with("INVALID_ARGUMENT"), "{refused}");
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R007.1, MEDULLA-FINANCE-PRIMITIVES-R006.1
    #[test]
    fn the_strategy_evaluate_op_proposes_a_buy_and_refuses_a_non_positive_amount() {
        let spec = |amount: i64| StrategySpec {
            version: STRATEGY_SPEC_VERSION,
            kind: StrategyKind::DollarCostAverageFixedAmount {
                amount_minor_units: amount,
            },
        };
        let action = decoded::<ProposedAction>(handle(
            2,
            FinanceMarketOp::StrategyEvaluate { spec: spec(10_000) },
        ))
        .unwrap();
        assert_eq!(action.side, OrderSide::Buy);
        assert_eq!(action.amount_minor_units, 10_000);
        assert_eq!(action.strategy_version, STRATEGY_SPEC_VERSION);
        for amount in [0, -5] {
            let refused = decoded::<ProposedAction>(handle(
                3,
                FinanceMarketOp::StrategyEvaluate { spec: spec(amount) },
            ))
            .unwrap_err();
            assert!(refused.contains("INVALID_REQUEST"), "{refused}");
        }
    }
}
