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
    // An admitted node write commits through the authoritative MutationBatch,
    // which needs a persistence backend.
    #[cfg(feature = "redb")]
    {
        state.write().await.persistence = Some(std::sync::Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                medulla_server_api::api::dispatch_support::unique_temp_dir(
                    "medulla-client-session-dispatch",
                )
                .to_string_lossy()
                .into_owned(),
                256,
            )
            .expect("open client-session test redb backend"),
        ));
    }
    // The graph ACL admits only registered principals with write on the
    // shared graph; both callers write their session node there, so the
    // principal rule alone decides each case.
    for agent_id in ["usr:ann", "usr:root"] {
        state
            .write()
            .await
            .isolation
            .register_agent(medulla_types::acl::AgentIdentity {
                agent_id: agent_id.to_string(),
                role: medulla_types::acl::AgentRole::System,
                teams: Vec::new(),
                roles: Vec::new(),
            });
    }
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
            assert!(
                response.error.is_none(),
                "{node_id} {principal}: {response:?}"
            );
        }
    }
}
