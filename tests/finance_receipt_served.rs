//! MEDULLA-FINANCE-PRIMITIVES-R011.4: the `FinanceReceipt` idempotency record
//! through the real signed `dispatch` surface. `CreateNodeIfAbsent` is the
//! idempotent write; the caller computes the node id as the SHA-256 of the tenant
//! key, kind and idempotency key. A second identical write is a no-op and an
//! empty idempotency key refuses `RECEIPT_KEY_INVALID`.
//!
//! MEDULLA-FINANCE-PRIMITIVES-R011.5: the receipt then moves only through
//! `FinanceReceiptTransition` (a retry raises `attempt`), and `FinanceReceiptList` reads
//! the tenant's receipts under one tenant key.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_finance_types::finance_receipt::{
    FinanceReceiptEvent, FinanceReceiptPage, FinanceReceiptState, FinanceReceiptView,
};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use sha2::{Digest, Sha256};

const SECRET: &str = "medulla-fin-r011-4-served-receipt-secret";
const GRAPH: &str = "fin-r011-4";

fn receipt_id(tenant_key: &str, kind: &str, key: &str) -> String {
    let mut digest = Sha256::new();
    for part in [tenant_key, kind, key] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn receipt(key: &str) -> Method {
    let props = serde_json::json!({
        "type": "FinanceReceipt",
        "receiptKind": "alert_delivery",
        "receiptState": "due",
        "tenantKey": "tenant-1",
        "idempotencyKey": key,
        "receiptEventId": "event-1",
        "receiptMode": "live",
        "receiptChannel": "email",
        "receiptAttempt": 1,
        "recordedAt": "2026-10-09T00:00:00Z",
    });
    Method::CreateNodeIfAbsent {
        node_id: receipt_id("tenant-1", "alert_delivery", key),
        properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
    }
}

async fn call(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.4
#[tokio::test(flavor = "multi_thread")]
async fn finance_receipt_is_idempotent_by_key() {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let created = call(
        &state,
        2,
        Method::CreateGraph {
            graph_name: GRAPH.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await;
    assert!(created.error.is_none(), "{:?}", created.error_detail);

    let first = call(&state, 3, receipt("key-1")).await;
    assert!(first.error.is_none(), "{:?}", first.error_detail);
    assert!(matches!(first.result, Some(ResultPayload::Bool(true))));
    let second = call(&state, 4, receipt("key-1")).await;
    assert!(second.error.is_none(), "{:?}", second.error_detail);
    assert!(matches!(second.result, Some(ResultPayload::Bool(false))));

    let Method::CreateNodeIfAbsent { node_id, .. } = receipt("key-1") else {
        unreachable!()
    };
    let stored = call(&state, 5, Method::GetNodeProperties { node_id }).await;
    assert!(stored.error.is_none(), "{:?}", stored.error_detail);
    assert!(matches!(stored.result, Some(ResultPayload::Raw(_))));

    // An empty key refuses and writes nothing.
    let empty = call(&state, 6, receipt("")).await;
    assert!(empty.result.is_none(), "{empty:?}");
    assert!(
        empty
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("RECEIPT_KEY_INVALID"))
            || empty
                .error
                .as_deref()
                .is_some_and(|error| error.contains("RECEIPT_KEY_INVALID")),
        "{empty:?}"
    );
    let Method::CreateNodeIfAbsent { node_id, .. } = receipt("") else {
        unreachable!()
    };
    let absent = call(&state, 7, Method::GetNodeProperties { node_id }).await;
    assert!(
        !matches!(absent.result, Some(ResultPayload::Raw(ref bytes)) if !bytes.is_empty() && bytes.as_slice() != [0xc0]),
        "{absent:?}"
    );
}

fn view_of(response: &Response) -> FinanceReceiptView {
    assert!(response.error.is_none(), "{:?}", response.error_detail);
    let Some(ResultPayload::Raw(bytes)) = &response.result else {
        panic!("expected a raw receipt: {response:?}");
    };
    rmp_serde::from_slice(bytes).expect("a receipt view")
}

fn refused_with(response: &Response, code: &str) {
    assert!(response.result.is_none(), "{response:?}");
    assert!(
        response
            .error_detail
            .as_deref()
            .is_some_and(|d| d.contains(code))
            || response.error.as_deref().is_some_and(|e| e.contains(code)),
        "{response:?}"
    );
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
#[tokio::test(flavor = "multi_thread")]
async fn finance_receipt_moves_only_through_its_machine_and_lists_by_tenant_key() {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let graph = "fin-r011-5";
    let created = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            2,
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Global,
            },
        ),
    )
    .await;
    assert!(created.error.is_none(), "{:?}", created.error_detail);
    let call = |id: u64, method: Method| {
        let state = &state;
        async move {
            test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
        }
    };
    let Method::CreateNodeIfAbsent { node_id, .. } = receipt("key-1") else {
        unreachable!()
    };
    let transition = |event| Method::FinanceReceiptTransition {
        receipt_id: node_id.clone(),
        event,
    };

    // A receipt that does not start due at attempt 1 is refused and writes nothing.
    let started_claimed = {
        let props = serde_json::json!({
            "type": "FinanceReceipt", "receiptKind": "alert_delivery",
            "receiptState": "claimed", "tenantKey": "tenant-1", "idempotencyKey": "key-x",
            "receiptEventId": "e", "receiptMode": "live", "receiptChannel": "email",
            "receiptAttempt": 1, "recordedAt": "2026-10-09T00:00:00Z",
        });
        Method::CreateNodeIfAbsent {
            node_id: receipt_id("tenant-1", "alert_delivery", "key-x"),
            properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
        }
    };
    refused_with(&call(3, started_claimed).await, "RECEIPT_STATE_INVALID");

    assert!(call(4, receipt("key-1")).await.error.is_none());
    // Walk the machine: due, claimed, failed, then a retry raises the attempt.
    let claimed = view_of(&call(5, transition(FinanceReceiptEvent::Claim)).await);
    assert_eq!(claimed.receipt_state, FinanceReceiptState::Claimed);
    assert_eq!(claimed.attempt, 1);
    let failed = view_of(&call(6, transition(FinanceReceiptEvent::Fail)).await);
    assert_eq!(failed.receipt_state, FinanceReceiptState::Failed);
    let retried = view_of(&call(7, transition(FinanceReceiptEvent::Retry)).await);
    assert_eq!(retried.receipt_state, FinanceReceiptState::Due);
    assert_eq!(retried.attempt, 2);
    // An event no edge accepts is refused and leaves the receipt where it was.
    refused_with(
        &call(8, transition(FinanceReceiptEvent::Complete)).await,
        "TRANSITION_REFUSED",
    );
    // A second identical create does not reset the state.
    assert!(matches!(
        call(9, receipt("key-1")).await.result,
        Some(ResultPayload::Bool(false))
    ));
    // An absent receipt is the one RECEIPT_NOT_FOUND.
    let absent = Method::FinanceReceiptTransition {
        receipt_id: "no-such-receipt".to_string(),
        event: FinanceReceiptEvent::Claim,
    };
    refused_with(&call(10, absent).await, "RECEIPT_NOT_FOUND");

    // The list reads the tenant's receipts under one tenant key, filtered by state.
    assert!(call(11, receipt("key-2")).await.error.is_none());
    let list = |tenant_key: &str, state: Option<FinanceReceiptState>| Method::FinanceReceiptList {
        tenant_key: tenant_key.to_string(),
        state,
        kind: None,
        limit: None,
    };
    let page = |response: Response| -> FinanceReceiptPage {
        let Some(ResultPayload::Raw(bytes)) = response.result else {
            panic!("expected a page: {response:?}");
        };
        rmp_serde::from_slice(&bytes).expect("a receipt page")
    };
    let all = page(call(12, list("tenant-1", None)).await);
    assert_eq!(all.receipts.len(), 2);
    assert!(all.receipts.iter().all(|r| r.tenant_key == "tenant-1"));
    let due = page(call(13, list("tenant-1", Some(FinanceReceiptState::Due))).await);
    assert_eq!(due.receipts.len(), 2);
    let none = page(call(14, list("tenant-2", None)).await);
    assert!(none.receipts.is_empty());
}
