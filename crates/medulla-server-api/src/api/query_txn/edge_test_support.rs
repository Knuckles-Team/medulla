//! Served read-path fixtures for the edge-index status tests (MEDULLA-CONTRACT-R056.8).

/// The status query the three served read-path tests (Method::Sql,
/// KnowledgeStream, pgwire) all run against `medulla_index_status` (EH-352).
pub const EDGE_STATUS_SQL: &str = "SELECT index_name, target_kind, state \
    FROM information_schema.medulla_index_status WHERE target_kind = 'graph_edges'";

/// The edge index those tests register for `tenant`.
pub fn edge_status_spec(tenant: &str) -> medulla_query::edge_index::EdgeIndexSpec {
    medulla_query::edge_index::EdgeIndexSpec {
        name: "rel_emb".to_string(),
        property: "emb".to_string(),
        kind: medulla_query::edge_index::EdgeIndexKind::Vector {
            metric: medulla_query::VectorMetric::L2,
        },
        scope: medulla_query::edge_index::EdgeScope {
            tenant: tenant.to_string(),
            purpose: "retrieval".to_string(),
        },
    }
}

/// The row every path must answer for that index: registered durably, then
/// installed into the served graph by the read itself — `requested`, since
/// nothing built it in the served process yet.
pub fn edge_status_row() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!("rel_emb"),
        serde_json::json!("graph_edges"),
        serde_json::json!("requested"),
    ]
}

/// Register the edge index durably for `tenant` WITHOUT installing it into
/// the served graph (it is built on a detached core), so a read proves it
/// installs the tenant's edge indexes itself.
pub fn register_detached_edge_index(store: &medulla_query::TableStore, graph: &str, tenant: &str) {
    let detached = medulla_core::graph::GraphCore::new();
    medulla_query::edge_index::create_durable_edge_index(
        store,
        graph,
        &detached,
        edge_status_spec(tenant),
    )
    .expect("register the edge index durably");
}

/// The decoded rows of a served `QueryResult` response.
pub fn sql_rows(resp: &medulla_protocol_types::protocol::Response) -> Vec<Vec<serde_json::Value>> {
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = &resp.result else {
        panic!(
            "expected a Raw result, got {:?} / {:?}",
            resp.result, resp.error
        );
    };
    let result: medulla_protocol_types::protocol::QueryResult = rmp_serde::from_slice(bytes).expect("QueryResult");
    result
        .rows
        .iter()
        .map(|row| rmp_serde::from_slice(row).expect("row cells"))
        .collect()
}
