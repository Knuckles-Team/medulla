//! MEDULLA-FINANCE-PRIMITIVES-R011.4: the `FinanceReceipt` idempotency record
//! through the real signed `dispatch` surface. `CreateNodeIfAbsent` is the
//! idempotent write; the caller computes the node id as the SHA-256 of the tenant
//! key, kind, idempotency key and state. A second identical write is a no-op and an
//! empty idempotency key refuses `RECEIPT_KEY_INVALID`.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use sha2::{Digest, Sha256};

const SECRET: &str = "medulla-fin-r011-4-served-receipt-secret";
const GRAPH: &str = "fin-r011-4";

fn receipt_id(tenant_key: &str, kind: &str, key: &str, state: &str) -> String {
    let mut digest = Sha256::new();
    for part in [tenant_key, kind, key, state] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn receipt(key: &str) -> Method {
    let props = serde_json::json!({
        "type": "FinanceReceipt",
        "receiptKind": "alert_delivery",
        "receiptState": "claimed",
        "tenantKey": "tenant-1",
        "idempotencyKey": key,
        "recordedAt": "2026-10-09T00:00:00Z",
    });
    Method::CreateNodeIfAbsent {
        node_id: receipt_id("tenant-1", "alert_delivery", key, "claimed"),
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
    assert_eq!(first.result, Some(ResultPayload::Bool(true)));
    let second = call(&state, 4, receipt("key-1")).await;
    assert!(second.error.is_none(), "{:?}", second.error_detail);
    assert_eq!(second.result, Some(ResultPayload::Bool(false)));

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
