//! A stored pack row that no longer decodes is a typed refusal on every read
//! and is replaced by the next import (MEDULLA-TYPED-PACKS-R041.1, R041.2).

use medulla_catalog_types::connector_pack::{
    ConnectorPackOp, ConnectorPackStatus, ConnectorPackStatusRequest, PackWarningCode,
};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_types::contract::ResourceId;

use super::{bind, build_pack, imported, ok, refused, tool, Served, ADMIN, TENANT};

const CONNECTOR: &str = "pack-undecodable";

/// A row in a shape the current codec does not know.
fn foreign_shape() -> Vec<u8> {
    let row = std::collections::BTreeMap::from([("retired_shape_field", 1u8)]);
    rmp_serde::to_vec_named(&row).unwrap()
}

async fn plant(served: &Served, key: &str, nonce: u8, member_uri: Option<&str>) {
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let context = medulla_persistence_records::agent_fixtures::mutation_context(
        &store,
        TENANT,
        key,
        nonce,
        0,
        "connector-pack:bind",
    );
    store
        .connector_pack_plant_test_row(
            &context,
            &ResourceId::new(CONNECTOR).unwrap(),
            member_uri,
            &foreign_shape(),
        )
        .unwrap();
}

async fn status(served: &Served, key: &str) -> medulla_protocol_types::protocol::Response {
    served
        .pack(
            key,
            ConnectorPackOp::Status {
                request: ConnectorPackStatusRequest {
                    tenant_id: TENANT.to_string(),
                    connector: ResourceId::new(CONNECTOR).unwrap(),
                },
            },
        )
        .await
}

// spec: MEDULLA-TYPED-PACKS-R041.1
// spec: MEDULLA-TYPED-PACKS-R041.2
#[tokio::test]
async fn an_undecodable_stored_pack_refuses_typed_and_a_reimport_replaces_it() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    imported(
        &served,
        &build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]),
        None,
    )
    .await;
    plant(&served, "plant-head", 201, None).await;
    plant(
        &served,
        "plant-member",
        202,
        Some("tool://pack-undecodable/stale"),
    )
    .await;

    // R041.1: the served read names the typed code and carries the detail.
    let response = status(&served, "status-undecodable").await;
    let detail = response.error_detail.clone().unwrap_or_default();
    assert_eq!(
        refused("the undecodable head", response),
        "PACK_RECORD_UNDECODABLE"
    );
    assert!(detail.contains(&format!("'{CONNECTOR}'")), "{detail}");
    assert!(detail.contains("does not decode: "), "{detail}");

    // R041.2: a fresh import replaces the undecodable rows, audited on the receipt.
    let receipt = imported(
        &served,
        &build_pack(
            CONNECTOR,
            &[
                tool(CONNECTOR, "a", "Tool a."),
                tool(CONNECTOR, "b", "Tool b."),
            ],
        ),
        None,
    )
    .await;
    assert!(
        receipt
            .warnings
            .iter()
            .any(|warning| warning.code == PackWarningCode::UndecodableRecordReplaced),
        "{:?}",
        receipt.warnings
    );
    let current: ConnectorPackStatus = ok("Status", status(&served, "status-replaced").await);
    assert_eq!(
        current.head.map(|head| head.pack_digest),
        Some(receipt.pack_digest)
    );
}

// spec: MEDULLA-TYPED-PACKS-R041.1
#[tokio::test]
async fn an_undecodable_head_hides_its_pack_and_never_fails_the_component_search() {
    use medulla_agent_types::agent_component::{
        AgentComponentKind, AgentComponentOp, AgentComponentSearchPage, AgentComponentSearchRequest,
    };
    use medulla_protocol_types::protocol::Method;

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    imported(
        &served,
        &build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]),
        None,
    )
    .await;
    let search = |key: &'static str| {
        let served = &served;
        async move {
            served
                .call(
                    key,
                    Method::AgentComponent {
                        op: AgentComponentOp::Search {
                            request: AgentComponentSearchRequest {
                                tenant_id: TENANT.to_string(),
                                task: None,
                                capabilities: Vec::new(),
                                kinds: vec![AgentComponentKind::Tool],
                                read_only: false,
                                limit: None,
                                cursor: None,
                            },
                        },
                    },
                )
                .await
        }
    };
    let before: AgentComponentSearchPage = ok("Search", search("search-before").await);
    assert_eq!(before.entries.len(), 1, "the imported tool is listed");

    plant(&served, "plant-head-search", 203, None).await;

    // The search answers (it used to refuse with a code the method does not
    // declare, which surfaced as an unclassified INTERNAL), and the pack whose
    // head cannot be read is dark until a re-import replaces it.
    let after: AgentComponentSearchPage = ok("Search", search("search-after").await);
    assert!(after.entries.is_empty(), "{:?}", after.entries);
    // The connector's own status read still names the typed refusal.
    assert_eq!(
        refused("status", status(&served, "status-after-search").await),
        "PACK_RECORD_UNDECODABLE"
    );
}
