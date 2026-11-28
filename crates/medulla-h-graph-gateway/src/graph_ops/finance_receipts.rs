//! Served finance receipts (MEDULLA-FINANCE-PRIMITIVES-R011.4, R011.5).
//!
//! A receipt is created through `CreateNodeIfAbsent`: [`admit_create`] checks the one initial
//! shape (`due`, attempt 1, a non-empty idempotency key) and stamps the verified tenant scope
//! onto the node, so every later read and transition is tenant scoped. After that the state
//! moves only through `FinanceReceiptTransition`, which applies `FINANCE_RECEIPT_MACHINE`; a
//! `Retry` raises the stored attempt in the same write. `FinanceReceiptList` is the terminal
//! read over the RLS-projected core: the receipts of the caller's tenant scope under one
//! tenant key. A receipt of another tenant is the same `RECEIPT_NOT_FOUND` as an absent one.

use super::*;

use medulla_finance_types::finance_receipt::{
    FinanceReceiptPage, FinanceReceiptState, FinanceReceiptView,
    FINANCE_RECEIPT_LIST_DEFAULT_LIMIT, FINANCE_RECEIPT_LIST_MAX_LIMIT, FINANCE_RECEIPT_MACHINE,
};

use super::ops_context::GraphOpsContext;

/// The node property naming the opaque verified tenant scope of the receipt's writer.
const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
/// The node class (and label) of a receipt.
const RECEIPT_CLASS: &str = "FinanceReceipt";
/// The keyset page the list scans the label index in.
const SCAN_PAGE: usize = 1024;

type Properties = serde_json::Map<String, serde_json::Value>;

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn not_found(receipt_id: &str) -> String {
    refuse(
        "RECEIPT_NOT_FOUND",
        format!("no receipt {receipt_id} under this tenant"),
    )
}

fn is_receipt(props: &Properties) -> bool {
    [props.get("type"), props.get("finance_kind")]
        .into_iter()
        .flatten()
        .any(|class| class.as_str() == Some(RECEIPT_CLASS))
}

/// The create write of a `FinanceReceipt`, admitted: `None` when the write is not a receipt,
/// otherwise the refusal or the same write with the tenant scope stamped. A receipt starts in
/// the machine's initial state at attempt 1, so the machine is the only way to move it.
pub fn admit_create(tenant_scope: &str, method: &Method) -> Option<Result<Method, String>> {
    let Method::CreateNodeIfAbsent {
        node_id,
        properties_msgpack,
    } = method
    else {
        return None;
    };
    let mut props = medulla_types::msgpack::decode_property_object(properties_msgpack).ok()?;
    if !is_receipt(&props) {
        return None;
    }
    Some(stamp_new(node_id, tenant_scope, &mut props))
}

fn stamp_new(node_id: &str, tenant_scope: &str, props: &mut Properties) -> Result<Method, String> {
    let key_present = props
        .get("idempotencyKey")
        .and_then(|value| value.as_str())
        .is_some_and(|key| !key.trim().is_empty());
    if !key_present {
        return Err(refuse(
            "RECEIPT_KEY_INVALID",
            "a FinanceReceipt needs a non-empty idempotencyKey",
        ));
    }
    let starts_due = props.get("receiptState").and_then(|v| v.as_str())
        == Some(medulla_types::transition_refused::MachineState::stored(
            FINANCE_RECEIPT_MACHINE.initial,
        ))
        && props.get("receiptAttempt").and_then(|v| v.as_u64()) == Some(1);
    if !starts_due {
        return Err(refuse(
            "RECEIPT_STATE_INVALID",
            "a new FinanceReceipt starts due at attempt 1; later states come from a transition",
        ));
    }
    props.insert(
        TENANT_SCOPE_KEY.to_owned(),
        serde_json::Value::String(tenant_scope.to_owned()),
    );
    let properties_msgpack = rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: finance receipt encoding failed: {error}"))?;
    Ok(Method::CreateNodeIfAbsent {
        node_id: node_id.to_owned(),
        properties_msgpack,
    })
}

/// The stored receipt `receipt_id` of `tenant_scope`; an absent, foreign or non-receipt node
/// is the one `RECEIPT_NOT_FOUND`.
fn stored_receipt(
    core: &GraphCore,
    tenant_scope: &str,
    receipt_id: &str,
) -> Result<Properties, String> {
    core.get_node_properties(receipt_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .filter(|props| {
            is_receipt(props)
                && props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) == Some(tenant_scope)
        })
        .ok_or_else(|| not_found(receipt_id))
}

/// Apply one event to the stored receipt: the machine's next state and attempt, the node
/// properties to write and the view to answer.
fn admit_transition(
    core: &GraphCore,
    tenant_scope: &str,
    method: &Method,
) -> Result<(Vec<u8>, FinanceReceiptView), String> {
    let Method::FinanceReceiptTransition { receipt_id, event } = method else {
        unreachable!("admit_transition is only called for FinanceReceiptTransition");
    };
    let mut props = stored_receipt(core, tenant_scope, receipt_id)?;
    let current = FinanceReceiptView::from_properties(receipt_id, &props).ok_or_else(|| {
        refuse(
            "RECEIPT_STATE_INVALID",
            format!("{receipt_id} is not a complete receipt"),
        )
    })?;
    let (state, attempt) = current
        .receipt_state
        .step(current.attempt, *event)
        .map_err(|refused| refused.to_string())?;
    props.insert(
        "receiptState".to_owned(),
        serde_json::Value::String(
            medulla_types::transition_refused::MachineState::stored(state).to_owned(),
        ),
    );
    props.insert(
        "receiptAttempt".to_owned(),
        serde_json::Value::from(attempt),
    );
    let view = FinanceReceiptView::from_properties(receipt_id, &props)
        .ok_or_else(|| not_found(receipt_id))?;
    let blob = rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: finance receipt encoding failed: {error}"))?;
    Ok((blob, view))
}

/// The `Route::FinanceRecords` entry for `FinanceReceiptTransition`.
pub async fn transition(ctx: &MutationCtx<'_>, method: &Method) -> Option<Response> {
    if !matches!(method, Method::FinanceReceiptTransition { .. }) {
        return None;
    }
    if let Err(error) = admit_transition(ctx.core, ctx.tenant_scope, method) {
        return Some(Response::err(ctx.req_id, error));
    }
    let plan = MutationPlan::for_method(method);
    let (scope, owned) = (ctx.tenant_scope.to_string(), method.clone());
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, method, move |core| {
            // Re-admitted under the graph lane, so a competing transition that committed
            // since the check above moves the machine from its new state, never from a stale one.
            let (blob, view) = admit_transition(core, &scope, &owned)?;
            core.add_node(view.receipt_id.clone(), blob);
            ResultPayload::of::<
                medulla_protocol_types::result_contract::graph::FinanceReceiptTransition,
            >(view)
        })
        .await,
    )
}

/// `FinanceReceiptList` over the RLS-projected core (`terminal.rs`).
pub async fn handle_receipt_list(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::FinanceReceiptList {
        tenant_key,
        state,
        kind,
        limit,
    } = &method
    else {
        return ControlFlow::Continue(method);
    };
    let Some(scope) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            "ACCESS_DENIED: a receipt list needs a verified tenant",
        ));
    };
    if tenant_key.trim().is_empty() {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            "INVALID_ARGUMENT: a receipt list names a non-empty tenant_key",
        ));
    }
    let limit = limit.unwrap_or(FINANCE_RECEIPT_LIST_DEFAULT_LIMIT);
    if limit == 0 || limit > FINANCE_RECEIPT_LIST_MAX_LIMIT {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            format!(
                "INVALID_ARGUMENT: a receipt list limit is 1 to {FINANCE_RECEIPT_LIST_MAX_LIMIT}"
            ),
        ));
    }
    let wanted_state: Option<FinanceReceiptState> = *state;
    let mut receipts: Vec<FinanceReceiptView> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = ctx
            .core
            .get_nodes_by_label_page(RECEIPT_CLASS, after.as_deref(), SCAN_PAGE);
        let full = page.len() == SCAN_PAGE;
        after = page.last().map(|(id, _)| id.clone());
        for (id, blob) in &page {
            let Some(props) = medulla_types::msgpack::decode_property_object(blob).ok() else {
                continue;
            };
            if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) != Some(scope.as_str()) {
                continue;
            }
            let Some(view) = FinanceReceiptView::from_properties(id, &props) else {
                continue;
            };
            if view.tenant_key == *tenant_key
                && wanted_state.is_none_or(|wanted| view.receipt_state == wanted)
                && kind
                    .as_deref()
                    .is_none_or(|wanted| view.receipt_kind == wanted)
            {
                receipts.push(view);
            }
        }
        if !full || after.is_none() {
            break;
        }
    }
    receipts.sort_by(|a, b| (&a.recorded_at, &a.receipt_id).cmp(&(&b.recorded_at, &b.receipt_id)));
    receipts.truncate(limit as usize);
    let page = FinanceReceiptPage { receipts };
    ControlFlow::Break(
        match ResultPayload::of::<medulla_protocol_types::result_contract::graph::FinanceReceiptList>(
            page,
        ) {
            Ok(payload) => Response::ok(ctx.req_id, payload),
            Err(error) => Response::err(ctx.req_id, error),
        },
    )
}
