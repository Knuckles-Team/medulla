//! `Method::FinanceSignalModels`: Bayesian signal fusion and the strategic
//! insider under dynamic legal risk (EH-423 / AUD-30).
//!
//! Pure compute over what the request carries; every op answers its declared
//! result or a typed refusal rendered `"CODE: detail"`.

use medulla_finance_quant::{
    signal_models::bayes_fuse, signal_models::insider_equilibrium,
    signal_models::FinanceSignalModelsOp,
};
use medulla_protocol_types::result_contract::compute as results;

use super::market::answer;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;

pub fn handle(req_id: u64, op: FinanceSignalModelsOp) -> Response {
    match op {
        FinanceSignalModelsOp::BayesFuse { request } => {
            answer::<results::FinanceSignalModelsBayesFuse>(req_id, bayes_fuse(&request))
        }
        FinanceSignalModelsOp::InsiderEquilibrium { request } => {
            answer::<results::FinanceSignalModelsInsiderEquilibrium>(
                req_id,
                insider_equilibrium(&request),
            )
        }
    }
}

/// The route-family entry: a `FinanceSignalModels` request, or the method handed back.
pub(super) fn handle_signal_models(req_id: u64, method: Method) -> Result<Response, Method> {
    match method {
        Method::FinanceSignalModels { op } => Ok(handle(req_id, *op)),
        other => Err(other),
    }
}
