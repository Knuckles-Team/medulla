//! The production search source port of the freshness refresh, over sources registered
//! through the served `RegisterForeignSource` method (MEDULLA-DECISION-ENGINE-R174.9.3.1).

use medulla_h_decide::decide::stat_freshness::{SearchSourcePort, SEARCH_SOURCE_NAME};
use medulla_h_decide::decide::stat_test_support::Harness;
use medulla_protocol_types::protocol::{Method, Request};
use medulla_query_types::wire::ForeignSourceSpec;
use medulla_server::freshness_search_port::FederationSearchPort;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_ops::api::query_txn::ForeignStateExt;

const TENANT: &str = "tenant-one";
const OTHER_TENANT: &str = "tenant-two";

fn web_spec(server: &str) -> ForeignSourceSpec {
    ForeignSourceSpec::Mcp {
        server: server.into(),
        tool: "web_search".into(),
        capabilities: Default::default(),
    }
}

async fn register(h: &Harness, tenant: &str, agent: &str, name: &str, source: ForeignSourceSpec) {
    let response = Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 1749,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some(agent.into()),
                method: Method::RegisterForeignSource {
                    name: name.into(),
                    quota: None,
                    source,
                    acceleration: None,
                },
            },
            VerifiedRequestContext::verified_for_test_with_scopes(agent, tenant, &["*"]),
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "RegisterForeignSource was refused: {:?}",
        response.refusal_text()
    );
}

fn owner_scope(agent: &str, tenant: &str) -> String {
    CarrierAuthority::from_verified(&VerifiedRequestContext::verified_for_test_in_tenant(
        agent, tenant,
    ))
    .expect("verified carrier")
    .owner_scope()
    .to_string()
}

// spec: MEDULLA-DECISION-ENGINE-R174.9.3.1
#[tokio::test]
async fn registered_search_source_binds_and_unregistered_does_not() {
    let h = Harness::new().await;
    let catalog = h.state.read().await.foreign_sources();
    let port = FederationSearchPort::new(std::sync::Arc::clone(&catalog));

    // Nothing is registered: no tenant binds.
    assert!(port.binding(TENANT, "__commons__").is_none());

    register(&h, TENANT, "agent-a", SEARCH_SOURCE_NAME, web_spec("search-one")).await;
    let binding = port
        .binding(TENANT, "__commons__")
        .expect("the registered web source binds for its tenant");
    assert_eq!(binding.connection_iri, SEARCH_SOURCE_NAME);
    assert_eq!(binding.tenant, TENANT);
    // The same source serves every graph of the tenant, and no other tenant binds.
    assert!(port.binding(TENANT, "another-graph").is_some());
    assert!(port.binding(OTHER_TENANT, "__commons__").is_none());

    // A source of the tenant under another name is not the search source.
    register(&h, OTHER_TENANT, "agent-c", "papers", web_spec("papers")).await;
    assert!(port.binding(OTHER_TENANT, "__commons__").is_none());
    // A registered source that is not a web metasearch source binds nothing.
    register(
        &h,
        OTHER_TENANT,
        "agent-c",
        SEARCH_SOURCE_NAME,
        ForeignSourceSpec::HttpJson {
            url: "http://example.invalid/papers".into(),
            json_path: "data".into(),
            field_map: medulla_query_types::wire::HttpFieldMap {
                id: "id".into(),
                score: None,
                columns: Default::default(),
            },
        },
    )
    .await;
    assert!(port.binding(OTHER_TENANT, "__commons__").is_none());

    // A second principal of the same tenant registering `search` does not change which
    // spec is returned when its owner scope is higher.
    let (low, high) = if owner_scope("agent-a", TENANT) < owner_scope("agent-b", TENANT) {
        ("agent-a", "agent-b")
    } else {
        ("agent-b", "agent-a")
    };
    register(&h, TENANT, low, SEARCH_SOURCE_NAME, web_spec("search-low")).await;
    register(&h, TENANT, high, SEARCH_SOURCE_NAME, web_spec("search-high")).await;
    assert_eq!(
        catalog.spec_for_tenant(TENANT, SEARCH_SOURCE_NAME),
        Some(web_spec("search-low"))
    );
    assert!(port.binding(TENANT, "__commons__").is_some());
}
