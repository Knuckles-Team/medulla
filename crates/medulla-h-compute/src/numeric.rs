//! `Method::NumericEvaluate`: the served numeric kernel evaluation
//! (MEDULLA-FINANCE-PRIMITIVES-R002.2.5).
//!
//! Pure compute over what the request carries: no store, no clock, no graph core.
//! The handler hands the op to `medulla_numeric::evaluation::registry::evaluate`
//! and encodes the answer as the declared result of that op. A refusal is
//! `INVALID_ARGUMENT` with the stable `KERNEL_*` code first in the detail.

#![allow(clippy::result_large_err)]

use medulla_numeric::evaluation::registry;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::compute as results;
use medulla_types::numeric_kernel::{NumericEvaluateOp as Op, NumericKernelOutput as Out};

fn mismatch(req_id: u64, op: &Op) -> Response {
    Response::err(
        req_id,
        format!(
            "INTERNAL: the registry answered `{}` with a result of another shape",
            op.name()
        ),
    )
}

/// Encode `out` as the declared result of `op`.
fn encode(req_id: u64, op: &Op, out: Out) -> Response {
    let payload = match (op, out) {
        (Op::RollingZScore { .. }, Out::Series { values }) => {
            ResultPayload::of::<results::NumericEvaluateRollingZScore>(values)
        }
        (Op::Ewma { .. }, Out::Series { values }) => {
            ResultPayload::of::<results::NumericEvaluateEwma>(values)
        }
        (Op::InformationCoefficient { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateInformationCoefficient>(value))
        }
        (Op::InformationRatio { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateInformationRatio>(value))
        }
        (Op::EffectiveIndependentN { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateEffectiveIndependentN>(value))
        }
        (Op::DieboldMariano { .. }, Out::DieboldMariano(v)) => {
            ResultPayload::of::<results::NumericEvaluateDieboldMariano>(v)
        }
        (Op::DeflatedSharpe { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateDeflatedSharpe>(value))
        }
        (Op::PurgedCpcv { .. }, Out::Splits { splits }) => {
            ResultPayload::of::<results::NumericEvaluatePurgedCpcv>(splits)
        }
        (Op::ProbabilityOfBacktestOverfit { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateProbabilityOfBacktestOverfit>(value))
        }
        (Op::Brier { .. }, Out::Scalar { value }) => {
            Ok(ResultPayload::scalar::<results::NumericEvaluateBrier>(value))
        }
        (Op::KalmanLevel { .. }, Out::Kalman(v)) => {
            ResultPayload::of::<results::NumericEvaluateKalmanLevel>(v)
        }
        (Op::KalmanBeta { .. }, Out::Kalman(v)) => {
            ResultPayload::of::<results::NumericEvaluateKalmanBeta>(v)
        }
        (Op::KalmanVolatility { .. }, Out::Vector { values }) => {
            ResultPayload::of::<results::NumericEvaluateKalmanVolatility>(values)
        }
        (Op::OuCalibrate { .. }, Out::OuParams(v)) => {
            ResultPayload::of::<results::NumericEvaluateOuCalibrate>(v)
        }
        (Op::OuThresholds { .. }, Out::OuThresholds(v)) => {
            ResultPayload::of::<results::NumericEvaluateOuThresholds>(v)
        }
        (Op::Adf { .. }, Out::Adf(v)) => ResultPayload::of::<results::NumericEvaluateAdf>(v),
        (Op::RegimeHmm { .. }, Out::Regime(v)) => {
            ResultPayload::of::<results::NumericEvaluateRegimeHmm>(v)
        }
        (op, _) => return mismatch(req_id, op),
    };
    Response::ok(req_id, payload)
}

/// Serve one `NumericEvaluate` op.
pub fn handle_numeric_evaluate(req_id: u64, op: &Op) -> Response {
    match registry::evaluate(op) {
        Ok(out) => encode(req_id, op, out),
        Err(refusal) => Response::err(req_id, format!("INVALID_ARGUMENT: {refusal}")),
    }
}

/// The route entry: a `NumericEvaluate` request, or the method handed back.
pub fn try_handle(req_id: u64, method: Method) -> Result<Response, Method> {
    match method {
        Method::NumericEvaluate { op } => Ok(handle_numeric_evaluate(req_id, &op)),
        other => Err(other),
    }
}
