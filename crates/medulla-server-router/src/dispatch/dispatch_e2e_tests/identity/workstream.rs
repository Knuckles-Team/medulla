//! `Workstream` node writes through the served boundary
//! (MEDULLA-IDENTITY-R723): the `principal` must be the verified caller, with
//! no scope lifting the rule, and the node id must name the caller's tenant.

use super::*;
use medulla_server_api::client_session::{workstream_node_id, WORKSTREAM_LABEL};

const TENANT: &str = "tenant-shared";
const WRITE_SCOPE: &str = "kg:write";
const DIGEST: &str = "sha256-0f3a";

fn workstream_write(node_id: &str, principal: &str, last_active_ms: u64) -> Method {
    let properties = serde_json::json!({
        "type": WORKSTREAM_LABEL,
        "principal": principal,
        "tenant": TENANT,
        "workspace_digest": DIGEST,
        "opened_at_ms": 1u64,
        "last_active_ms": last_active_ms,
    });
    Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&properties).expect("encode"),
    }
}

fn mismatch(response: &Response) -> bool {
    response.error.as_deref() == Some("WORKSTREAM_PRINCIPAL_MISMATCH")
}

#[tokio::test]
async fn workstream_attach_round_trip_and_principal_mismatch_refused() {
    // spec: MEDULLA-IDENTITY-R723
    let state = state();
    #[cfg(feature = "redb")]
    {
        state.write().await.persistence = Some(std::sync::Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                medulla_server_api::api::dispatch_support::unique_temp_dir(
                    "medulla-workstream-dispatch",
                )
                .to_string_lossy()
                .into_owned(),
                256,
            )
            .expect("open workstream test redb backend"),
        ));
    }
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
    let own_id = workstream_node_id(TENANT, DIGEST);
    let other_tenant_id = workstream_node_id("tenant-other", DIGEST);
    let caller = || context("usr:ann", &[WRITE_SCOPE]);
    let admin = || context("usr:root", &[WRITE_SCOPE, IDENTITY_ADMIN_SCOPE]);
    // (caller, node id, principal property, refused). A second attach is the
    // same upsert with a later last_active_ms.
    let cases = [
        (caller(), &own_id, "usr:ann", 1u64, false),
        (caller(), &own_id, "usr:ann", 2u64, false),
        (caller(), &own_id, "usr:bob", 3u64, true),
        (caller(), &other_tenant_id, "usr:ann", 3u64, true),
        (admin(), &own_id, "usr:bob", 3u64, true),
    ];
    for (context, node_id, principal, last_active_ms, refused) in cases {
        let response = send_method(
            &state,
            context,
            workstream_write(node_id, principal, last_active_ms),
        )
        .await;
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
