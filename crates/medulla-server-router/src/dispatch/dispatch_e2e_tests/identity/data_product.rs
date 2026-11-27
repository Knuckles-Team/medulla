//! MEDULLA-TYPED-PACKS-R805.3: a served access request is decided by the product owner or a
//! steward of its datasets, the approval writes the sharing grant through the audited share
//! path, and a caller with neither role or another tenant is refused.

use super::*;
use medulla_catalog_types::data_product::{
    AccessRequest, DataContract, DataProduct, ProductPort, ProductPortKind,
};
use medulla_catalog_types::stewardship::{
    StewardRole, StewardshipAssignment, StewardshipOp, StewardshipScope,
};
use medulla_types::access_request_machine::AccessRequestState;

const TENANT: &str = "tenant-shared";
const OWNER: &str = "usr:owner";
const CONSUMER: &str = "usr:consumer";
const STEWARD: &str = "usr:steward";

async fn durable_state() -> Arc<RwLock<ServerState>> {
    let state = state();
    #[cfg(feature = "redb")]
    {
        state.write().await.persistence = Some(std::sync::Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                medulla_server_api::api::dispatch_support::unique_temp_dir(
                    "medulla-data-product-dispatch",
                )
                .to_string_lossy()
                .into_owned(),
                256,
            )
            .expect("open data product test redb backend"),
        ));
    }
    state
}

fn product(id: &str, dataset: &str) -> (DataProduct, DataContract) {
    (
        DataProduct {
            id: id.to_string(),
            owner: OWNER.to_string(),
            version: "1.0.0".to_string(),
            datasets: vec![dataset.to_string()],
            terms: Vec::new(),
            ports: vec![ProductPort {
                name: "sql".to_string(),
                kind: ProductPortKind::Query,
            }],
        },
        DataContract {
            product: id.to_string(),
            schema_digest: format!("sha256:{}", "a".repeat(64)),
            quality_rules: Vec::new(),
            freshness_s: 3600,
        },
    )
}

fn request(product: &str) -> AccessRequest {
    AccessRequest {
        product: product.to_string(),
        principal: CONSUMER.to_string(),
        purpose: "monthly revenue report".to_string(),
        state: AccessRequestState::Pending,
        grant_ref: None,
    }
}

async fn add_node(state: &Arc<RwLock<ServerState>>, node_id: &str, value: serde_json::Value) {
    let created = send_method(
        state,
        context(OWNER, &["*"]),
        Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&value).expect("encode"),
        },
    )
    .await;
    assert!(created.error.is_none(), "{:?}", created.error);
}

async fn grants_of(state: &Arc<RwLock<ServerState>>, node_id: &str) -> serde_json::Value {
    let response = send_method(
        state,
        context(OWNER, &["*"]),
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let properties = match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            medulla_types::msgpack::decode_property_object(&bytes).expect("node properties")
        }
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => object,
        other => panic!("expected node properties, got {other:?}"),
    };
    properties
        .get("_grants")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

async fn register(state: &Arc<RwLock<ServerState>>, id: &str, dataset: &str) {
    let (product, contract) = product(id, dataset);
    let registered = send_method(
        state,
        context(OWNER, &["*"]),
        Method::DataProductRegister {
            tenant: TENANT.to_string(),
            product,
            contract,
        },
    )
    .await;
    assert!(registered.error.is_none(), "{:?}", registered.error);
}

async fn open(state: &Arc<RwLock<ServerState>>, id: &str) {
    let opened = send_method(
        state,
        context(CONSUMER, &["*"]),
        Method::AccessRequestOpen {
            tenant: TENANT.to_string(),
            request: request(id),
        },
    )
    .await;
    assert!(opened.error.is_none(), "{:?}", opened.error);
}

fn decide(id: &str, approve: bool) -> Method {
    Method::AccessRequestDecide {
        tenant: TENANT.to_string(),
        product: id.to_string(),
        principal: CONSUMER.to_string(),
        approve,
    }
}

// spec: MEDULLA-TYPED-PACKS-R805.3
#[tokio::test(flavor = "multi_thread")]
async fn request_grants_access_after_steward_approval() {
    let state = durable_state().await;
    // The owner's dataset is private; the steward's dataset is shared with the steward only.
    add_node(
        &state,
        "ds-owned",
        serde_json::json!({"type": "Dataset", "_owner_id": OWNER}),
    )
    .await;
    add_node(
        &state,
        "ds-stewarded",
        serde_json::json!({
            "type": "Dataset",
            "_owner_id": OWNER,
            "_shared_scope": "org",
            "_grants": [STEWARD],
        }),
    )
    .await;
    register(&state, "revenue", "ds-owned").await;
    register(&state, "orders", "ds-stewarded").await;
    let assigned = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::Stewardship {
            op: Box::new(StewardshipOp::Assign {
                assignment: StewardshipAssignment {
                    scope: StewardshipScope::Asset("ds-stewarded".to_string()),
                    role: StewardRole::Steward,
                    principal: STEWARD.to_string(),
                },
            }),
        },
    )
    .await;
    assert!(assigned.error.is_none(), "{:?}", assigned.error);
    open(&state, "revenue").await;
    open(&state, "orders").await;

    // A caller with neither role is refused and nothing changes.
    let refused = send_method(&state, context(CONSUMER, &["*"]), decide("revenue", true)).await;
    assert!(
        refused
            .refusal_text()
            .is_some_and(|text| text.contains("STEWARD_ROLE_MISSING")),
        "{:?}",
        refused.error
    );
    assert_eq!(grants_of(&state, "ds-owned").await, serde_json::Value::Null);

    // The product owner approves: the dataset's grants name the consumer.
    let approved = send_method(&state, context(OWNER, &["*"]), decide("revenue", true)).await;
    assert!(approved.error.is_none(), "{:?}", approved.error);
    assert_eq!(
        grants_of(&state, "ds-owned").await,
        serde_json::json!([CONSUMER])
    );

    // A steward of the dataset approves the other product.
    let approved = send_method(&state, context(STEWARD, &["*"]), decide("orders", true)).await;
    assert!(approved.error.is_none(), "{:?}", approved.error);
    assert_eq!(
        grants_of(&state, "ds-stewarded").await,
        serde_json::json!([CONSUMER, STEWARD])
    );

    // An approved request is not decided twice.
    let again = send_method(&state, context(OWNER, &["*"]), decide("revenue", false)).await;
    assert!(
        again
            .refusal_text()
            .is_some_and(|text| text.contains("TRANSITION_REFUSED")),
        "{:?}",
        again.error
    );
}

// spec: MEDULLA-TYPED-PACKS-R805.3
#[tokio::test(flavor = "multi_thread")]
async fn a_steward_approves_over_a_private_node_owned_by_someone_else() {
    let state = durable_state().await;
    // Nobody but its owner (a third principal) can read this node under row-level security.
    add_node(
        &state,
        "ds-private",
        serde_json::json!({"type": "Dataset", "_owner_id": "usr:someone-else"}),
    )
    .await;
    add_node(
        &state,
        "ds-other",
        serde_json::json!({"type": "Dataset", "_owner_id": "usr:someone-else"}),
    )
    .await;
    register(&state, "ledger", "ds-private").await;
    let assigned = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::Stewardship {
            op: Box::new(StewardshipOp::Assign {
                assignment: StewardshipAssignment {
                    scope: StewardshipScope::Asset("ds-private".to_string()),
                    role: StewardRole::Steward,
                    principal: STEWARD.to_string(),
                },
            }),
        },
    )
    .await;
    assert!(assigned.error.is_none(), "{:?}", assigned.error);
    open(&state, "ledger").await;

    // The product owner does not own the node and holds no stewardship of it: still hidden.
    let owner = send_method(&state, context(OWNER, &["*"]), decide("ledger", true)).await;
    assert!(
        owner.error.is_some(),
        "the owner of the product is no steward of the node"
    );
    assert_eq!(
        grants_of(&state, "ds-private").await,
        serde_json::Value::Null
    );

    // The steward the store authorized for the node approves: the engine carries the read and
    // the write, and the node names the consumer.
    let approved = send_method(&state, context(STEWARD, &["*"]), decide("ledger", true)).await;
    assert!(approved.error.is_none(), "{:?}", approved.error);
    assert_eq!(
        grants_of(&state, "ds-private").await,
        serde_json::json!([CONSUMER])
    );
    // A node the steward does not govern is untouched.
    assert_eq!(grants_of(&state, "ds-other").await, serde_json::Value::Null);
}

// spec: MEDULLA-TYPED-PACKS-R805.3
#[tokio::test(flavor = "multi_thread")]
async fn product_methods_refuse_another_tenant() {
    let state = durable_state().await;
    let (product, contract) = product("revenue", "ds-owned");
    let foreign = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::DataProductRegister {
            tenant: "tenant-other".to_string(),
            product,
            contract,
        },
    )
    .await;
    assert!(
        foreign
            .refusal_text()
            .is_some_and(|text| text.contains("RECORD_TENANT_MISMATCH")),
        "{:?}",
        foreign.error
    );
}

// spec: MEDULLA-TYPED-PACKS-R805.3
#[tokio::test(flavor = "multi_thread")]
async fn a_product_version_change_opens_a_change_request_for_the_dataset_stewards() {
    let state = durable_state().await;
    add_node(
        &state,
        "ds-owned",
        serde_json::json!({"type": "Dataset", "_owner_id": OWNER}),
    )
    .await;
    register(&state, "revenue", "ds-owned").await;
    let assigned = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::Stewardship {
            op: Box::new(StewardshipOp::Assign {
                assignment: StewardshipAssignment {
                    scope: StewardshipScope::Asset("ds-owned".to_string()),
                    role: StewardRole::Steward,
                    principal: STEWARD.to_string(),
                },
            }),
        },
    )
    .await;
    assert!(assigned.error.is_none(), "{:?}", assigned.error);
    let (mut next, mut contract) = product("revenue", "ds-owned");
    next.version = "2.0.0".to_string();
    contract.schema_digest = format!("sha256:{}", "c".repeat(64));
    let registered = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::DataProductRegister {
            tenant: TENANT.to_string(),
            product: next,
            contract,
        },
    )
    .await;
    assert!(registered.error.is_none(), "{:?}", registered.error);
    let listed = send_method(
        &state,
        context(OWNER, &["*"]),
        Method::GetNodesByLabel {
            label: "cat:ChangeRequest".to_string(),
            after: None,
            limit: 10,
        },
    )
    .await;
    let Some(ResultPayload::NodeList(nodes)) = listed.result else {
        panic!("no change request listed: {:?}", listed.error);
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].1["target"], serde_json::json!("revenue"));
    assert_eq!(nodes[0].1["reviewers"], serde_json::json!([STEWARD]));
}

fn refusal_of(response: &Response) -> String {
    response.refusal_text().unwrap_or_default()
}

// spec: MEDULLA-TYPED-PACKS-R805.4
#[tokio::test(flavor = "multi_thread")]
async fn product_refusals() {
    let state = durable_state().await;
    add_node(
        &state,
        "ds-owned",
        serde_json::json!({"type": "Dataset", "_owner_id": OWNER}),
    )
    .await;
    register(&state, "revenue", "ds-owned").await;

    // An empty purpose is refused under its own code, and nothing is stored.
    let mut empty = request("revenue");
    empty.purpose = "  ".to_string();
    let refused = send_method(
        &state,
        context(CONSUMER, &["*"]),
        Method::AccessRequestOpen {
            tenant: TENANT.to_string(),
            request: empty,
        },
    )
    .await;
    assert!(
        refusal_of(&refused).contains("ACCESS_PURPOSE_MISSING"),
        "{:?}",
        refused.error
    );
    let unstored = send_method(&state, context(OWNER, &["*"]), decide("revenue", true)).await;
    assert!(
        refusal_of(&unstored).contains("no access request"),
        "{:?}",
        unstored.error
    );

    // A caller that is not the product owner cannot register it.
    let (held_product, held_contract) = product("revenue", "ds-owned");
    let stranger = send_method(
        &state,
        context(CONSUMER, &["*"]),
        Method::DataProductRegister {
            tenant: TENANT.to_string(),
            product: held_product,
            contract: held_contract,
        },
    )
    .await;
    assert!(
        refusal_of(&stranger).contains("STEWARD_ROLE_MISSING"),
        "{:?}",
        stranger.error
    );

    // A new version that changes the contract schema needs a higher major version.
    let broken = |version: &str| {
        let (mut product, mut contract) = product("revenue", "ds-owned");
        product.version = version.to_string();
        contract.schema_digest = format!("sha256:{}", "b".repeat(64));
        Method::DataProductRegister {
            tenant: TENANT.to_string(),
            product,
            contract,
        }
    };
    let minor = send_method(&state, context(OWNER, &["*"]), broken("1.1.0")).await;
    assert!(
        refusal_of(&minor).contains("CONTRACT_SCHEMA_BREAK"),
        "{:?}",
        minor.error
    );
    let major = send_method(&state, context(OWNER, &["*"]), broken("2.0.0")).await;
    assert!(major.error.is_none(), "{:?}", major.error);
}
