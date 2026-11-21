//! MEDULLA-IDENTITY-R023.13.3: a served `RbacAdmin` share commits the node's share
//! keys through the graph gateway, and the same keys read back unchanged.

use super::*;
use medulla_identity_types::acl::{RbacAdminOp, ShareGrant, ShareRevoke};
use medulla_identity_types::identity::ShareScope;

const NODE: &str = "share-node-1";
const OWNER: &str = "usr:owner";

fn owner() -> VerifiedRequestContext {
    context(OWNER, &["*"])
}

async fn properties(state: &Arc<RwLock<ServerState>>) -> serde_json::Map<String, serde_json::Value> {
    let response = send_method(
        state,
        owner(),
        Method::GetNodeProperties {
            node_id: NODE.to_string(),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            medulla_types::msgpack::decode_property_object(&bytes).expect("node properties")
        }
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => object,
        other => panic!("expected node properties, got {other:?}"),
    }
}

// spec: MEDULLA-IDENTITY-R023.13.3
#[tokio::test(flavor = "multi_thread")]
async fn share_grant_commits_through_gateway() {
    let state = state();
    let seeded = serde_json::json!({"type": "Note", "_owner_id": OWNER});
    let created = send_method(
        &state,
        owner(),
        Method::AddNode {
            node_id: NODE.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&seeded).expect("encode"),
        },
    )
    .await;
    assert!(created.error.is_none(), "{:?}", created.error);

    let grant = ShareGrant::new(
        NODE.to_string(),
        ShareScope::Tenant,
        vec!["usr:b".to_string(), "usr:a".to_string()],
    )
    .expect("a valid grant");
    let granted = send_method(
        &state,
        owner(),
        Method::RbacAdmin {
            op: RbacAdminOp::ShareGrant(grant),
        },
    )
    .await;
    assert!(granted.error.is_none(), "{:?}", granted.error);
    let shared = properties(&state).await;
    assert_eq!(shared["_shared_scope"], "org");
    assert_eq!(shared["_grants"], serde_json::json!(["usr:a", "usr:b"]));
    assert_eq!(shared["_owner_id"], OWNER);
    assert_eq!(shared["type"], "Note");

    let revoked = send_method(
        &state,
        owner(),
        Method::RbacAdmin {
            op: RbacAdminOp::ShareRevoke(ShareRevoke::new(NODE.to_string()).expect("a node id")),
        },
    )
    .await;
    assert!(revoked.error.is_none(), "{:?}", revoked.error);
    let private = properties(&state).await;
    assert_eq!(private["_shared_scope"], "private");
    assert_eq!(private["_grants"], serde_json::json!([]));
}
