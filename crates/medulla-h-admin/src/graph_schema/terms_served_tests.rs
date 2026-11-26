//! `GraphSchemaClasses` over the served dispatch path (EH-389 visibility ruling,
//! 2026-09-24): a page shows declaring source keys, but only those of the REQUEST
//! graph's own schema-source set -- the exact set `GraphSchemaList` lists -- and a
//! request from another tenant never reaches the handler.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_catalog_types::graph_schema::{GraphSchemaClassesView, GraphSchemaSourcesView};
use medulla_graph_core::graph::GraphSchemaSource;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_graph_core::graph::SchemaSourceOrigin;
use medulla_protocol_types::protocol::GraphType;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::build_current_test_request;
use medulla_server_auth::auth::dispatch_test_on_heap;

const SECRET: &str = "graph-schema-classes-served-secret";
const AGENT: &str = "schema-vocabulary-reader";
const TENANT: &str = "tenant-shared";

fn admin_sources(name: &str, class_iri: &str) -> Arc<GraphSchemaSources> {
    let ontology = format!("<{class_iri}> a <http://www.w3.org/2002/07/owl#Class> .\n");
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Admin {
            name: name.to_string(),
        },
        None,
        Some(Arc::from(ontology.as_str())),
        0,
    )
    .expect("a bounded admin source");
    let dynamic = BTreeMap::from([(format!("admin:{name}"), source)]);
    Arc::new(GraphSchemaSources::with_dynamic(dynamic).expect("composes with the core"))
}

async fn state_with_two_graphs() -> Arc<RwLock<ServerState>> {
    let state = medulla_server::seams_install::new_state_for_test(
        SECRET,
        ServerState::test_isolation(AGENT),
    );
    let state = Arc::new(RwLock::new(state));
    {
        let mut guard = state.write().await;
        for (graph, name, iri) in [
            ("vocab-a", "alpha", "https://example.org/a#OnlyInA"),
            ("vocab-b", "beta", "https://example.org/b#OnlyInB"),
        ] {
            guard
                .registry
                .create_graph(graph, GraphType::Global, None)
                .expect("create graph");
            let core = guard.registry.get(graph).expect("resident").core.clone();
            core.install_schema_sources(admin_sources(name, iri));
        }
    }
    state
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    graph: &str,
    method: Method,
) -> Response {
    let request = build_current_test_request(SECRET, tenant, 1, graph, AGENT, method);
    dispatch_test_on_heap(state, request).await
}

fn raw<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("decodes"),
        other => panic!("expected a Raw result, got {other:?}"),
    }
}

fn classes() -> Method {
    Method::GraphSchemaClasses {
        kind: None,
        cursor: None,
        limit: 1000,
    }
}

async fn every_term(state: &Arc<RwLock<ServerState>>, graph: &str) -> Vec<(String, String)> {
    let mut terms = Vec::new();
    let mut cursor = None;
    loop {
        let method = Method::GraphSchemaClasses {
            kind: None,
            cursor: cursor.take(),
            limit: 1000,
        };
        let page: GraphSchemaClassesView = raw(&call(state, TENANT, graph, method).await);
        terms.extend(
            page.terms
                .as_slice()
                .iter()
                .map(|term| (term.iri.clone(), term.source_id.clone())),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return terms,
        }
    }
}

#[tokio::test]
async fn a_page_shows_only_the_request_graphs_own_sources() {
    let state = state_with_two_graphs().await;
    let terms = every_term(&state, "vocab-a").await;
    assert!(terms.contains(&(
        "https://example.org/a#OnlyInA".to_string(),
        "admin:alpha".to_string()
    )));
    assert!(
        terms
            .iter()
            .all(|(iri, source)| !iri.contains("/b#") && source != "admin:beta"),
        "graph B's source must never appear on graph A"
    );
    let listed: GraphSchemaSourcesView =
        raw(&call(&state, TENANT, "vocab-a", Method::GraphSchemaList).await);
    let listed: Vec<&str> = listed
        .core_sources
        .as_slice()
        .iter()
        .chain(listed.dynamic_sources.as_slice())
        .map(|source| source.source_id.as_str())
        .collect();
    assert!(
        terms
            .iter()
            .all(|(_, source)| listed.contains(&source.as_str())),
        "every declaring source is one GraphSchemaList shows for the same graph"
    );
}

#[tokio::test]
async fn a_request_from_another_tenant_is_refused() {
    let state = state_with_two_graphs().await;
    let response = call(&state, "tenant-foreign", "vocab-a", classes()).await;
    let error = response.error.expect("a foreign tenant is refused");
    assert!(
        !error.contains("OnlyInA") && !error.contains("admin:alpha"),
        "{error}"
    );
    assert!(response.result.is_none());
}

/// MEDULLA-TYPED-PACKS-R812.2: a served page carries each class's direct `rdfs:subClassOf`
/// IRIs, so a client rolls a pack class up to the core vocabulary it extends.
// spec: MEDULLA-TYPED-PACKS-R812.2
#[tokio::test]
async fn a_served_class_term_carries_its_direct_superclasses() {
    const CORE_DOCUMENT: &str = "https://knuckles-team.github.io/ontology/core#Document";
    let ontology = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         <https://example.org/c#Report> a owl:Class ; rdfs:subClassOf <{CORE_DOCUMENT}> .\n\
         <https://example.org/c#Memo> a owl:Class ; rdfs:subClassOf <https://example.org/c#Report> .\n\
         <https://example.org/c#Free> a owl:Class .\n"
    );
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Admin {
            name: "tree".to_string(),
        },
        None,
        Some(Arc::from(ontology.as_str())),
        0,
    )
    .expect("a bounded admin source");
    let state = medulla_server::seams_install::new_state_for_test(
        SECRET,
        ServerState::test_isolation(AGENT),
    );
    let state = Arc::new(RwLock::new(state));
    {
        let mut guard = state.write().await;
        guard
            .registry
            .create_graph("vocab-tree", GraphType::Global, None)
            .expect("create graph");
        let core = guard.registry.get("vocab-tree").expect("resident").core.clone();
        core.install_schema_sources(Arc::new(
            GraphSchemaSources::with_dynamic(BTreeMap::from([(
                "admin:tree".to_string(),
                source,
            )]))
            .expect("composes with the core"),
        ));
    }
    let method = Method::GraphSchemaClasses {
        kind: Some(medulla_catalog_types::graph_schema::GraphSchemaTermKind::Class),
        cursor: None,
        limit: 1000,
    };
    let page: GraphSchemaClassesView = raw(&call(&state, TENANT, "vocab-tree", method).await);
    let superclasses = |iri: &str| {
        page.terms
            .as_slice()
            .iter()
            .find(|term| term.iri == iri)
            .unwrap_or_else(|| panic!("{iri} is listed"))
            .superclasses
            .clone()
    };
    assert_eq!(
        superclasses("https://example.org/c#Report"),
        [CORE_DOCUMENT.to_string()]
    );
    assert_eq!(
        superclasses("https://example.org/c#Memo"),
        ["https://example.org/c#Report".to_string()],
        "direct superclasses only, not the transitive closure"
    );
    assert!(superclasses("https://example.org/c#Free").is_empty());
}

/// Verifies MEDULLA-TYPED-PACKS-R813: `GraphSchemaGet` answers one source's stored documents
/// through the same disclosed set `GraphSchemaList` shows, refuses a key the request graph does
/// not carry (a key of another graph included) by code, and both reads are `owl:read`.
// spec: MEDULLA-TYPED-PACKS-R813
#[tokio::test]
async fn get_serves_one_source_of_the_request_graph_and_refuses_the_rest() {
    use medulla_catalog_types::graph_schema::GraphSchemaSourceDocumentsView;

    let state = state_with_two_graphs().await;
    let get = |source_id: &str| Method::GraphSchemaGet {
        source_id: source_id.to_string(),
    };
    let found: GraphSchemaSourceDocumentsView =
        raw(&call(&state, TENANT, "vocab-a", get("admin:alpha")).await);
    assert_eq!(found.graph, "vocab-a");
    assert_eq!(found.source.source_id, "admin:alpha");
    assert!(found
        .ontology_ttl
        .as_deref()
        .is_some_and(|ttl| ttl.contains("https://example.org/a#OnlyInA")));
    assert_eq!(found.shapes_ttl, None);
    assert_eq!(found.source.ontology_bytes as usize, found.ontology_ttl.unwrap().len());

    // The list of the same graph names the same source with the same projection and digest.
    let listed: GraphSchemaSourcesView =
        raw(&call(&state, TENANT, "vocab-a", Method::GraphSchemaList).await);
    assert_eq!(listed.composed_digest, found.composed_digest);
    assert!(listed
        .dynamic_sources
        .as_slice()
        .iter()
        .any(|source| source == &found.source));

    // Another graph's key and an absent key refuse alike.
    for key in ["admin:beta", "admin:absent"] {
        let refused = call(&state, TENANT, "vocab-a", get(key)).await;
        assert_eq!(refused.error.as_deref(), Some("SCHEMA_SOURCE_NOT_FOUND"), "{key}");
    }
    // A core source is readable: the composition's reference vocabulary is part of the graph.
    let core_key = listed
        .core_sources
        .as_slice()
        .first()
        .map(|source| source.source_id.clone())
        .expect("the engine core catalog attaches at least one source");
    let core: GraphSchemaSourceDocumentsView =
        raw(&call(&state, TENANT, "vocab-a", get(&core_key)).await);
    assert_eq!(core.source.source_id, core_key);

    // A request from another tenant never reaches the handler.
    let foreign = call(&state, "tenant-foreign", "vocab-a", get("admin:alpha")).await;
    assert!(foreign.error.is_some());
    assert!(foreign.result.is_none());
}
