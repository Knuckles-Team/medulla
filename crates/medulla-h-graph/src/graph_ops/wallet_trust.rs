//! `Method::WalletKeyTrustGet` (MEDULLA-FINANCE-PRIMITIVES-R904): the current
//! wallet key trust record of one binding under the caller's verified tenant.
//!
//! The read runs over the row-level-security projected core. Records are
//! `WalletKeyTrust` nodes written through the `AddNode` gateway route; only
//! records whose `tenant_id` is the verified tenant are candidates, so a
//! cross-tenant read answers `WALLET_KEY_TRUST_NOT_FOUND`.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_types::contract::ServerErrorCode;
use medulla_types::finance_wallet_trust::{current_record, WalletKeyTrust, WALLET_KEY_TRUST_LABEL};

/// Page size of the label scan; every page is read.
const PAGE: usize = 1024;

/// Route the finance record reads served from the graph.
pub(super) async fn try_handle_finance(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    match method {
        Method::WalletKeyTrustGet {
            account_id,
            wallet_id,
            network_id,
        } => ControlFlow::Break(get(ctx, &account_id, &wallet_id, &network_id)),
        other => ControlFlow::Continue(other),
    }
}

fn not_found(req_id: u64) -> Response {
    Response::err(
        req_id,
        format!(
            "{}: no wallet key trust record for this binding",
            ServerErrorCode::WalletKeyTrustNotFound.as_str()
        ),
    )
}

/// Read every `WalletKeyTrust` node of the projected core.
fn records(core: &GraphCore) -> Vec<WalletKeyTrust> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = core.get_nodes_by_label_page(WALLET_KEY_TRUST_LABEL, after.as_deref(), PAGE);
        let full = page.len() == PAGE;
        after = page.last().map(|(id, _)| id.clone());
        out.extend(page.into_iter().filter_map(|(_, blob)| {
            let value = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            serde_json::from_value::<WalletKeyTrust>(value).ok()
        }));
        if !full || after.is_none() {
            return out;
        }
    }
}

/// The current record for the binding, or not found.
pub(super) fn get(
    ctx: GraphOpsContext<'_>,
    account_id: &str,
    wallet_id: &str,
    network_id: &str,
) -> Response {
    let Some(carrier) = ctx.read_authority.carrier() else {
        return not_found(ctx.req_id);
    };
    let tenant = carrier.log_owner().0;
    let all = records(ctx.core);
    match current_record(&all, tenant, account_id, wallet_id, network_id) {
        Some(record) => Response::ok(
            ctx.req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::graph::WalletKeyTrustGet,
            >(record),
        ),
        None => not_found(ctx.req_id),
    }
}
