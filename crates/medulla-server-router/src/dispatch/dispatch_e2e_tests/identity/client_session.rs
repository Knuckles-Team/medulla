//! `ClientSession` node writes through the served boundary
//! (MEDULLA-IDENTITY-R705): the `principal` must be the verified caller unless
//! the caller holds `identity:admin`, and the node id must name the caller's
//! tenant.

use super::*;
use medulla_server_api::client_session::{client_session_node_id, CLIENT_SESSION_LABEL};

const TENANT: &str = "tenant-shared";
const WRITE_SCOPE: &str = "kg:write";

fn session_write(node_id: &str, principal: &str) -> Method {
    let properties = serde_json::json!({
        "type": CLIENT_SESSION_LABEL,
        "principal": principal,
        "tenant": TENANT,
        "session_id": "s1",
        "client": "arbora-mcp",
        "started_at_ms": 1u64,
    });
    Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&properties).expect("encode"),
    }
}

fn mismatch(response: &Response) -> bool {
    response.error.as_deref() == Some("CLIENT_SESSION_PRINCIPAL_MISMATCH")
}

#[tokio::test]
async fn client_session_node_round_trip_and_principal_mismatch_refused() {
    // spec: MEDULLA-IDENTITY-R705
    let state = state();
    let own_id = client_session_node_id(TENANT, "s1");
    let other_tenant_id = client_session_node_id("tenant-other", "s1");
    let caller = || context("usr:ann", &[WRITE_SCOPE]);
    let admin = || context("usr:root", &[WRITE_SCOPE, IDENTITY_ADMIN_SCOPE]);
    // (caller, node id, principal property, refused)
    let cases = [
        (caller(), &own_id, "usr:ann", false),
        (caller(), &own_id, "usr:bob", true),
        (caller(), &other_tenant_id, "usr:ann", true),
        (admin(), &own_id, "usr:bob", false),
    ];
    for (context, node_id, principal, refused) in cases {
        let response = send_method(&state, context, session_write(node_id, principal)).await;
        assert_eq!(
            mismatch(&response),
            refused,
            "{node_id} {principal}: {response:?}"
        );
        if !refused {
            assert!(response.error.is_none(), "{:?}", response.error);
        }
    }
}
