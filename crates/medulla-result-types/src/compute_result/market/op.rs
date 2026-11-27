//! The operations of the `FinanceMarket` method. Every one is pure compute over
//! what the request carries: no store, no clock, no order authority.

use serde::{Deserialize, Serialize};

use super::bars::{BarRecord, FinalityFilter, SeriesPoint, Timeframe, TradingCalendar};
use super::chart::DecimateRequest;
use super::corporate_action::CorporateAction;
use super::evidence::{BacktestRunDraft, FlipConfidenceRequest};
use super::liquidity::LiquidityPanelRequest;
use super::scan::ScanRequest;
use super::signal::{IndicatorSpec, SignalCheckpoint, SignalReplayRequest};
use super::snapshot::AnalysisSnapshotDraft;
use super::strategy::StrategySpec;

/// One market-signal operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FinanceMarketOp {
    /// Bar records as time-series store points (the `TsAppend` layout).
    EncodePoints { records: Vec<BarRecord> },
    /// The latest version of every bar known as of `as_of` (every version
    /// when absent), ordered by open time, over `records` together with
    /// time-series store `points` decoded and validated in the bar layout.
    Resolve {
        #[serde(default)]
        records: Vec<BarRecord>,
        #[serde(default)]
        points: Vec<SeriesPoint>,
        #[serde(default)]
        as_of: Option<i64>,
        finality: FinalityFilter,
    },
    /// Resolved bars rolled up to a coarser calendar timeframe; a rolled bar is
    /// final only when every part is final and `watermark` has passed its close.
    Rollup {
        bars: Vec<BarRecord>,
        calendar: TradingCalendar,
        timeframe: Timeframe,
        watermark: i64,
    },
    /// One indicator over resolved final bars, one value per bar.
    Indicators {
        bars: Vec<BarRecord>,
        spec: IndicatorSpec,
    },
    /// Replay every bar version of one series through a trailing-trend signal.
    SignalReplay { request: SignalReplayRequest },
    /// Advance an indicator checkpoint by new final bars after its last one, in O(1) each.
    SignalAdvance {
        checkpoint: SignalCheckpoint,
        bars: Vec<BarRecord>,
    },
    /// The latest-state scanner view over many signal states.
    SignalScan { request: ScanRequest },
    /// Decide-calibrated follow-through for one flip, or an abstention.
    FlipConfidence { request: FlipConfidenceRequest },
    /// Validate a backtest run and seal it as a content-addressed record.
    BacktestRun { draft: BacktestRunDraft },
    /// Bars and aligned indicator series thinned by M4 to a pixel width.
    Decimate { request: DecimateRequest },
    /// Versioned point-in-time net-liquidity composite and heat strip.
    LiquidityPanel { request: LiquidityPanelRequest },
    /// Validate an analysis and seal it as a content-addressed snapshot with
    /// its notices; re-sealing a stored draft verifies the record.
    AnalysisSnapshot { draft: AnalysisSnapshotDraft },
    /// The latest version of every corporate action over `records` known at or before `as_of`
    /// (every version when absent), ordered by listing then effective time; two different
    /// bodies at one revision refuse (MEDULLA-FINANCE-PRIMITIVES-R006).
    CorporateActions {
        records: Vec<CorporateAction>,
        #[serde(default)]
        as_of: Option<i64>,
    },
    /// Evaluate one versioned strategy specification into a proposed action; a spec that
    /// proposes none refuses `INVALID_REQUEST`. Never places or authorizes an order
    /// (MEDULLA-FINANCE-PRIMITIVES-R007.1).
    StrategyEvaluate { spec: StrategySpec },
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R906.2
    #[test]
    fn signal_advance_carries_a_checkpoint_not_a_state() {
        let old = r#"{"op":"signal_advance","state":{},"bars":[]}"#;
        let error = serde_json::from_str::<FinanceMarketOp>(old)
            .expect_err("the old field name is refused");
        assert!(
            error.to_string().contains("unknown field `state`"),
            "{error}"
        );
        let new = r#"{"op":"signal_advance","checkpoint":{},"bars":[]}"#;
        let error = serde_json::from_str::<FinanceMarketOp>(new)
            .expect_err("an empty checkpoint is incomplete");
        assert!(
            !error.to_string().contains("unknown field `checkpoint`"),
            "{error}"
        );
    }
}
