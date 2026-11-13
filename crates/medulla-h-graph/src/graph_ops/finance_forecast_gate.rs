//! The deterministic-input gate of the finance sizing and backtest kernels
//! (MEDULLA-FINANCE-PRIMITIVES-R902).
//!
//! `FinancePositionSize` and the `BacktestRun` op of `FinanceMarket` accept a
//! deterministic series revision only. A request that names a revision through
//! `series_revision` (sizing) or a backtest draft's `data_revisions` is checked here
//! against the graph before the pure kernels run: when the caller can see the named
//! node and it is stored as a `ForecastSample`, the request is refused with
//! `INVALID_ARGUMENT: PROBABILISTIC_INPUT_REFUSED`. A revision that is not a stored
//! node (a bar-source digest) or is not visible to the caller is not a forecast and
//! passes through unchanged. Nothing is written.

use std::sync::Arc;

use medulla_finance_quant::sizing::SizingError;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_result_types::compute_result::market::FinanceMarketOp;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_types::finance_forecast::is_forecast_sample_class;

/// Whether `revision` names a visible stored node of the forecast sample class.
fn names_a_forecast(
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
    revision: &str,
) -> bool {
    if read_authority.is_some_and(|authority| !authority.node_visible(core, revision)) {
        return false;
    }
    core.get_node_properties(revision)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .is_some_and(|props| is_forecast_sample_class(props.get("type")))
}

/// The revision ids a sizing or backtest request names.
fn named_revisions(method: &Method) -> Vec<&str> {
    match method {
        Method::FinancePositionSize {
            series_revision, ..
        } => series_revision.as_deref().into_iter().collect(),
        Method::FinanceMarket { op } => match op.as_ref() {
            FinanceMarketOp::BacktestRun { draft } => draft
                .data_revisions
                .iter()
                .map(|revision| revision.source_revision.as_str())
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// `Some(refusal)` when the request names a probabilistic forecast as a kernel input.
pub fn refuse_probabilistic_input(
    req_id: u64,
    core: &Arc<GraphCore>,
    read_authority: Option<&GraphReadAuthority>,
    method: &Method,
) -> Option<Response> {
    named_revisions(method)
        .into_iter()
        .any(|revision| names_a_forecast(core, read_authority, revision))
        .then(|| {
            Response::err(
                req_id,
                format!("INVALID_ARGUMENT: {}", SizingError::ProbabilisticInput),
            )
        })
}
