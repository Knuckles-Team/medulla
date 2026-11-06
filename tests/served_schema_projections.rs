//! MEDULLA-FEDERATED-QUERY-R724: `SchemaProjections` through the real signed
//! `dispatch` surface. A `put` stores a tenant boundary (an unknown mapping is
//! refused), and a `list` returns only the boundaries the verified token's scopes
//! cover, never another tenant's, with the total count only for `audit:operator`.
#![cfg(all(feature = "server", feature = "security", feature = "query"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_catalog_types::schema_boundary::{
    SchemaBoundary, SchemaProjections, SchemaProjectionsOp, SCHEMA_BOUNDARY_LABEL,
    SCHEMA_BOUNDARY_TENANT_KEY,
};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use serde_json::json;

const SECRET: &str = "medulla-schema-projections-served-secret";
const GRAPH: &str = "schema-projections";
const MAPPING: &str = "urn:mapping:warehouse";

fn boundary(id: &str, scope: &str, mapping_ref: &str) -> SchemaBoundary {
    SchemaBoundary {
        boundary_id: id.into(),
        required_scopes: vec![scope.into()],
        tables: vec![format!("{id}_facts")],
        mapping_ref: mapping_ref.into(),
    }
}

fn op(op: SchemaProjectionsOp) -> Method {
    Method::SchemaProjections { op: Box::new(op) }
}

fn put(id: &str, scope: &str, mapping_ref: &str) -> Method {
    op(SchemaProjectionsOp::Put {
        boundary: boundary(id, scope, mapping_ref),
    })
}

fn node(node_id: &str, props: serde_json::Value) -> Method {
    Method::AddNode {
        node_id: node_id.into(),
        properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
    }
}

fn projections(response: &Response) -> SchemaProjections {
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        other => panic!(
            "expected projections, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FEDERATED-QUERY-R724
async fn projections_follow_caller_grants_and_hide_other_boundaries() {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let mut id = 0;
    let mut next = || {
        id += 1;
        id
    };
    let foreign = boundary("foreign", "finance:read", MAPPING);
    let setup = [
        Method::CreateGraph {
            graph_name: GRAPH.to_string(),
            graph_type: GraphType::Global,
        },
        node(MAPPING, json!({ "type": "vg:VirtualMapping" })),
        // Another tenant's boundary in the same graph: never a candidate.
        node(
            &SchemaBoundary::node_id("another-tenant-scope", "foreign"),
            json!({
                "type": SCHEMA_BOUNDARY_LABEL,
                SCHEMA_BOUNDARY_TENANT_KEY: "another-tenant-scope",
                "boundary": foreign,
            }),
        ),
        put("finance", "finance:read", MAPPING),
        put("blobs", "blob:read", MAPPING),
    ];
    for method in setup {
        let response =
            test_support::dispatch(&state, test_support::request(SECRET, next(), GRAPH, method))
                .await;
        assert!(
            response.error.is_none(),
            "setup: {:?}",
            response.error_detail
        );
    }

    let unknown = test_support::dispatch(
        &state,
        test_support::request(SECRET, next(), GRAPH, put("ghost", "blob:read", "urn:none")),
    )
    .await;
    assert_eq!(unknown.error.as_deref(), Some("BOUNDARY_MAPPING_UNKNOWN"));

    let list = |scopes: &[&str], id: u64| {
        let mut held = vec!["kg:read", "schema:projections"];
        held.extend_from_slice(scopes);
        common::signed_request_with_scopes(SECRET, id, GRAPH, &held, op(SchemaProjectionsOp::List))
    };
    let cases: &[(&[&str], &[&str], Option<u64>)] = &[
        (&["finance:read"], &["finance"], None),
        (&["blob:read", "finance:read"], &["blobs", "finance"], None),
        (&[], &[], None),
        (&["audit:operator"], &[], Some(2)),
    ];
    for (scopes, expected, count) in cases {
        let response = test_support::dispatch(&state, list(*scopes, next())).await;
        let out = projections(&response);
        let ids: Vec<&str> = out.entries.iter().map(|b| b.boundary_id.as_str()).collect();
        assert_eq!(ids, *expected, "scopes {scopes:?}");
        assert_eq!(out.boundary_count, *count, "scopes {scopes:?}");
        assert!(out.entries.iter().all(|b| b.mapping_ref == MAPPING));
    }
}
