//! MEDULLA-FINANCE-PRIMITIVES-R904: `WalletKeyTrustGet` returns the current wallet
//! key trust record of a binding under the caller's tenant through the real signed
//! `dispatch` surface; a record of another tenant is not found.
#![cfg(all(feature = "server", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use serde_json::{json, Value};

const SECRET: &str = "medulla-fin-wallet-trust-served-secret";
const GRAPH: &str = "fin-wallet-trust";
/// The verified tenant every `common::signed_request` carries.
const TENANT: &str = "integration-test-tenant";

fn record(node: &str, tenant: &str, wallet: &str, revision: u64, revoked: bool) -> Method {
    let props = json!({
        "type": "WalletKeyTrust",
        "tenant_id": tenant,
        "account_id": "acct-1",
        "wallet_id": wallet,
        "network_id": "net-main",
        "key_id": format!("key-{revision}"),
        "public_key": "a1".repeat(32),
        "signer_ref": "signer:hsm-1",
        "trust_revision": revision,
        "valid_from": "2026-01-01T00:00:00Z",
        "valid_until": null,
        "revoked": revoked,
    });
    Method::AddNode {
        node_id: node.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
    }
}

fn get(wallet: &str) -> Method {
    Method::WalletKeyTrustGet {
        account_id: "acct-1".into(),
        wallet_id: wallet.into(),
        network_id: "net-main".into(),
    }
}

fn body(response: &Response) -> Value {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => value.clone(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!(
            "expected a record, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R904
async fn get_returns_current_record_and_refuses_cross_tenant() {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let mut id = 0;
    let mut next = || {
        id += 1;
        id
    };
    let create = Method::CreateGraph {
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
    };
    for method in [
        create,
        record("wkt-1", TENANT, "w-1", 1, false),
        record("wkt-2", TENANT, "w-1", 2, true),
        record("wkt-x", "another-tenant", "w-2", 7, false),
    ] {
        let response =
            test_support::dispatch(&state, test_support::request(SECRET, next(), GRAPH, method))
                .await;
        assert!(
            response.error.is_none(),
            "setup: {:?}",
            response.error_detail
        );
    }

    let current = test_support::dispatch(
        &state,
        test_support::request(SECRET, next(), GRAPH, get("w-1")),
    )
    .await;
    let current = body(&current);
    assert_eq!(current["tenant_id"], TENANT);
    assert_eq!(current["account_id"], "acct-1");
    assert_eq!(current["wallet_id"], "w-1");
    assert_eq!(current["network_id"], "net-main");
    assert_eq!(current["key_id"], "key-2");
    assert_eq!(current["public_key"], "a1".repeat(32));
    assert_eq!(current["signer_ref"], "signer:hsm-1");
    assert_eq!(current["trust_revision"], 2);
    assert_eq!(current["valid_from"], "2026-01-01T00:00:00Z");
    assert_eq!(current["revoked"], true);

    let foreign = test_support::dispatch(
        &state,
        test_support::request(SECRET, next(), GRAPH, get("w-2")),
    )
    .await;
    assert!(foreign.result.is_none());
    assert_eq!(foreign.error.as_deref(), Some("WALLET_KEY_TRUST_NOT_FOUND"));
}
