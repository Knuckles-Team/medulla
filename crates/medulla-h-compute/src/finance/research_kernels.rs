//! Served finance research kernels: price-action pattern classification and
//! Brinson-Fachler profit attribution (MEDULLA-FINANCE-PRIMITIVES-R750.750 to R750.755).
//!
//! Pure compute over what the request carries; nothing is written. A refusal is
//! `INVALID_ARGUMENT: <code>: detail` with no partial result, rendered like
//! `numeric_kernels::refuse`.

use std::collections::BTreeSet;

use medulla_finance::attribution::brinson_fachler;
use medulla_finance_quant::pattern::{self, PatternKnobs};
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_protocol_types::result_contract::compute as results;
use medulla_result_types::compute_result::finance::GroupInput;
use medulla_server_api::knob_config;
use medulla_types::contract::ServerErrorCode;

use super::telemetry;

/// A refused research-kernel request. The refusal carries no partial result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResearchKernelError {
    /// Fewer than 2 bars.
    PatternWindowShort,
    /// Unequal slice lengths, a non-finite value, or `high < low`.
    PatternInputInvalid(&'static str),
    /// `w_p` or `w_b` does not sum to 1 within 1e-9, or a weight is negative.
    AttributionWeightsInvalid(&'static str),
    /// No groups, a duplicate group name, or a non-finite return.
    AttributionInputInvalid(&'static str),
}

impl ResearchKernelError {
    /// The stable wire code of this refusal.
    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::PatternWindowShort => ServerErrorCode::PatternWindowShort.as_str(),
            Self::PatternInputInvalid(_) => ServerErrorCode::PatternInputInvalid.as_str(),
            Self::AttributionWeightsInvalid(_) => {
                ServerErrorCode::AttributionWeightsInvalid.as_str()
            }
            Self::AttributionInputInvalid(_) => ServerErrorCode::AttributionInputInvalid.as_str(),
        }
    }

    fn detail(&self) -> &'static str {
        match self {
            Self::PatternWindowShort => "fewer than 2 bars",
            Self::PatternInputInvalid(d)
            | Self::AttributionWeightsInvalid(d)
            | Self::AttributionInputInvalid(d) => d,
        }
    }
}

fn refuse(req_id: u64, error: &ResearchKernelError) -> Response {
    Response::err(
        req_id,
        format!("INVALID_ARGUMENT: {}: {}", error.code(), error.detail()),
    )
}

fn validate_bars(
    open: &[f64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
) -> Result<(), ResearchKernelError> {
    let n = open.len();
    if high.len() != n || low.len() != n || close.len() != n {
        return Err(ResearchKernelError::PatternInputInvalid(
            "open, high, low and close differ in length",
        ));
    }
    if n < 2 {
        return Err(ResearchKernelError::PatternWindowShort);
    }
    let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
    if !(finite(open) && finite(high) && finite(low) && finite(close)) {
        return Err(ResearchKernelError::PatternInputInvalid(
            "a bar value is not finite",
        ));
    }
    if high.iter().zip(low).any(|(h, l)| h < l) {
        return Err(ResearchKernelError::PatternInputInvalid(
            "high is below low",
        ));
    }
    Ok(())
}

fn validate_groups(groups: &[GroupInput]) -> Result<(), ResearchKernelError> {
    if groups.is_empty() {
        return Err(ResearchKernelError::AttributionInputInvalid("no groups"));
    }
    let mut seen = BTreeSet::new();
    for g in groups {
        if !seen.insert(g.group.as_str()) {
            return Err(ResearchKernelError::AttributionInputInvalid(
                "duplicate group name",
            ));
        }
        if !(g.r_p.is_finite() && g.r_b.is_finite()) {
            return Err(ResearchKernelError::AttributionInputInvalid(
                "a return is not finite",
            ));
        }
    }
    if groups
        .iter()
        .any(|g| !(g.w_p.is_finite() && g.w_b.is_finite()) || g.w_p < 0.0 || g.w_b < 0.0)
    {
        return Err(ResearchKernelError::AttributionWeightsInvalid(
            "a weight is negative or not finite",
        ));
    }
    let sum_p: f64 = groups.iter().map(|g| g.w_p).sum();
    let sum_b: f64 = groups.iter().map(|g| g.w_b).sum();
    if (sum_p - 1.0).abs() > 1e-9 || (sum_b - 1.0).abs() > 1e-9 {
        return Err(ResearchKernelError::AttributionWeightsInvalid(
            "weights do not sum to 1 within 1e-9",
        ));
    }
    Ok(())
}

pub(super) fn handle_research_kernels(
    req_id: u64,
    tenant: &str,
    method: Method,
) -> Result<Response, Method> {
    let span = match &method {
        Method::FinancePatternClassify { .. } => {
            telemetry::kernel_span(telemetry::Kernel::PatternClassify, tenant, &method)
        }
        Method::FinanceProfitAttribution { .. } => {
            telemetry::kernel_span(telemetry::Kernel::ProfitAttribution, tenant, &method)
        }
        _ => return Err(method),
    };
    let _entered = span.enter();
    Ok(match method {
        Method::FinancePatternClassify {
            open,
            high,
            low,
            close,
        } => match validate_bars(&open, &high, &low, &close) {
            Ok(()) => {
                let k = knob_config::knobs();
                let knobs = PatternKnobs {
                    doji_body_max: k.pattern_doji_body_max,
                    shadow_ratio_min: k.pattern_shadow_ratio_min,
                    wick_max: k.pattern_wick_max,
                };
                let matches = pattern::classify(&open, &high, &low, &close, &knobs);
                telemetry::record_outcome(&span, telemetry::OUTCOME_OK);
                span.record("match_count", matches.len() as u64);
                Response::ok(
                    req_id,
                    ResultPayload::of::<results::FinancePatternClassify>(matches),
                )
            }
            Err(error) => {
                telemetry::record_outcome(&span, error.code());
                refuse(req_id, &error)
            }
        },
        Method::FinanceProfitAttribution { groups } => match validate_groups(&groups) {
            Ok(()) => {
                let breakdown = brinson_fachler(&groups);
                telemetry::record_outcome(&span, telemetry::OUTCOME_OK);
                span.record("active_return", breakdown.active_return);
                Response::ok(
                    req_id,
                    ResultPayload::of::<results::FinanceProfitAttribution>(breakdown),
                )
            }
            Err(error) => {
                telemetry::record_outcome(&span, error.code());
                refuse(req_id, &error)
            }
        },
        other => return Err(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_codes_are_registered_server_codes() {
        assert_eq!(
            ResearchKernelError::PatternWindowShort.code(),
            "PATTERN_WINDOW_SHORT"
        );
        assert_eq!(
            ResearchKernelError::AttributionWeightsInvalid("x").code(),
            "ATTRIBUTION_WEIGHTS_INVALID"
        );
    }
}
