//! Served proofs of graph-backed relational projections, read only
//! (MEDULLA-UNIFIED-DATA-PLANE-R026.4): `Method::ProjectionRead` through the real request
//! boundary, projection sessions on the real pgwire listener, and graph changes reaching
//! the projection only through the served mutation path.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "query",
    feature = "pgwire"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_types::projection_read::{
    result_digest, ProjectionCompare, ProjectionPredicate, ProjectionReadRequest,
    ProjectionReadResponse,
};
use medulla_wire::pgwire;

const SECRET: &str = "r026-4-projection-read-secret";
const GRAPH: &str = "__commons__";
const MAPPING: &str = "people";
const READER: &str = "projection-reader";
const MUTATOR: &str = "projection-mutator";

type Shared = Arc<RwLock<ServerState>>;

/// The served engine: the signing test agent sees the whole graph; `READER` holds a read
/// grant only and `MUTATOR` holds a read grant and a separate graph write grant.
fn state() -> Shared {
    use medulla_types::acl::{
        AgentIdentity, AgentRole, Grant, GrantEffect, RbacAction, ResourceSelector, Role,
    };
    let (persist_dir, persistence) = common::tempdir_persistence();
    let mut isolation = common::current_isolation();
    for (role, actions) in [
        ("projection-read-only", vec![RbacAction::Read]),
        (
            "projection-graph-writer",
            vec![RbacAction::Read, RbacAction::Write],
        ),
    ] {
        isolation.add_role(Role::new(role));
        for action in actions {
            isolation.add_grant(Grant {
                role: role.to_string(),
                resource: ResourceSelector::Graph(GRAPH.to_string()),
                action,
                effect: GrantEffect::Allow,
            });
        }
    }
    for (agent, role) in [
        (READER, "projection-read-only"),
        (MUTATOR, "projection-graph-writer"),
    ] {
        isolation.register_agent(AgentIdentity {
            agent_id: agent.to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: vec![role.to_string()],
        });
    }
    test_support::state_with(SECRET, isolation, persist_dir, persistence)
}

async fn call(state: &Shared, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

async fn put_node(state: &Shared, id: u64, node: &str, props: Value) -> Response {
    call(
        state,
        id,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
        },
    )
    .await
}

fn mapping_props(id: &str, revision: u64, state: &str) -> Value {
    json!({
        "type": "ProjectionMapping",
        "_visibility": "public",
        "mapping_id": id,
        "revision": revision,
        "approval_state": state,
        "tables": [{
            "name": "people",
            "label": "Person",
            "primary_key": "id",
            "columns": [
                {"name": "id", "field": "person_id", "column_type": "integer"},
                {"name": "name", "field": "full_name", "column_type": "text"},
                {"name": "age", "field": "age", "column_type": "integer"}
            ]
        }]
    })
}

fn person(id: i64, name: &str, age: i64) -> Value {
    json!({"type": "Person", "_visibility": "public", "person_id": id, "full_name": name, "age": age})
}

async fn put_mapping(state: &Shared, id: u64, mapping: &str, revision: u64, approval: &str) {
    let node = format!(
        "projection_mapping:{}:{mapping}:{revision}",
        common::TEST_TENANT
    );
    let response = put_node(state, id, &node, mapping_props(mapping, revision, approval)).await;
    assert!(response.error.is_none(), "mapping {node}: {response:?}");
}

async fn put_person(state: &Shared, id: u64, row: i64, name: &str, age: i64) {
    let response = put_node(state, id, &format!("person-{row}"), person(row, name, age)).await;
    assert!(response.error.is_none(), "person {row}: {response:?}");
}

async fn seed(state: &Shared) {
    put_mapping(state, 1, MAPPING, 1, "approved").await;
    put_person(state, 2, 10, "Ada", 36).await;
    put_person(state, 3, 2, "Grace", 45).await;
    put_person(state, 4, 7, "Linus", 28).await;
}

fn request_for(mapping: &str, revision: u64) -> ProjectionReadRequest {
    ProjectionReadRequest {
        tenant: common::TEST_TENANT.to_string(),
        actor: common::TEST_AGENT.to_string(),
        graph: GRAPH.to_string(),
        mapping_id: mapping.to_string(),
        mapping_revision: revision,
        table: "people".to_string(),
        columns: Vec::new(),
        predicates: Vec::new(),
        query_digest: String::new(),
    }
}

async fn read(state: &Shared, id: u64, request: ProjectionReadRequest) -> Response {
    call(
        state,
        id,
        Method::ProjectionRead {
            request: Box::new(request),
        },
    )
    .await
}

fn answer(response: &Response) -> ProjectionReadResponse {
    assert!(response.error.is_none(), "projection read: {response:?}");
    match &response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).expect("answer"),
        other => panic!("expected a projection answer, got {other:?}"),
    }
}

fn refused(response: &Response, code: &str) {
    assert!(response.result.is_none(), "a refused read returned rows");
    assert_eq!(response.error.as_deref(), Some(code), "{response:?}");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.4.1
#[tokio::test]
async fn projected_read_binds_revisions() {
    let state = state();
    seed(&state).await;

    let first = answer(&read(&state, 10, request_for(MAPPING, 1)).await);
    assert_eq!(first.mapping_revision, 1);
    assert!(first.graph_revision >= 4, "{first:?}");
    let ids: Vec<i64> = first
        .rows
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![2, 7, 10], "rows come back in primary-key order");
    assert_eq!(first.column_types.len(), 3);
    // The digest is SHA-256 over the canonical JSON of the rows, recomputed here.
    assert_eq!(first.result_digest.len(), 64);
    assert_eq!(first.result_digest, result_digest(&first.rows));
    assert!(first
        .result_digest
        .chars()
        .all(|c| matches!(c, '0'..='9' | 'a'..='f')));

    // The same read at the same revision gives the same answer.
    let again = answer(&read(&state, 11, request_for(MAPPING, 1)).await);
    assert_eq!(again.graph_revision, first.graph_revision);
    assert_eq!(again.result_digest, first.result_digest);

    // Columns and a typed predicate select a subset; the bound query digest is checked.
    let mut narrowed = request_for(MAPPING, 1);
    narrowed.columns = vec!["name".to_string()];
    narrowed.predicates = vec![ProjectionPredicate {
        column: "age".to_string(),
        op: ProjectionCompare::Gt,
        value: json!(30),
    }];
    narrowed.query_digest = narrowed.computed_query_digest();
    let subset = answer(&read(&state, 12, narrowed.clone()).await);
    let names: Vec<&str> = subset
        .rows
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Grace", "Ada"]);
    narrowed.query_digest = "0".repeat(64);
    let mismatch = read(&state, 13, narrowed).await;
    refused(&mismatch, "INVALID_ARGUMENT");

    // An unapproved revision, a model-suggested one and an unknown mapping are refused.
    put_mapping(&state, 20, "draft", 1, "proposed").await;
    refused(
        &read(&state, 21, request_for("draft", 1)).await,
        "PROJECTION_MAPPING_UNAPPROVED",
    );
    put_mapping(&state, 22, "guess", 1, "model-suggested").await;
    refused(
        &read(&state, 23, request_for("guess", 1)).await,
        "PROJECTION_MAPPING_UNAPPROVED",
    );
    refused(
        &read(&state, 24, request_for("never-registered", 1)).await,
        "PROJECTION_MAPPING_UNAPPROVED",
    );

    // A newer revision replaces the pinned one: the old pin is stale, the new one answers.
    put_mapping(&state, 25, MAPPING, 2, "approved").await;
    refused(
        &read(&state, 26, request_for(MAPPING, 1)).await,
        "PROJECTION_MAPPING_STALE",
    );
    assert_eq!(
        answer(&read(&state, 27, request_for(MAPPING, 2)).await).mapping_revision,
        2
    );

    // A read that is not the verified caller's own is denied.
    let mut foreign_tenant = request_for(MAPPING, 2);
    foreign_tenant.tenant = "another-tenant".to_string();
    refused(
        &read(&state, 28, foreign_tenant).await,
        "PROJECTION_ACCESS_DENIED",
    );
    let mut foreign_actor = request_for(MAPPING, 2);
    foreign_actor.actor = "someone-else".to_string();
    refused(
        &read(&state, 29, foreign_actor).await,
        "PROJECTION_ACCESS_DENIED",
    );
}

async fn listener(state: &Shared) -> String {
    test_support::spawn_pgwire_listener(state.clone(), pgwire::PgWireAuthMode::Scram).await
}

async fn projection_client(addr: &str, user: &str, mapping: &str) -> tokio_postgres::Client {
    let password = pgwire::derive_pg_password(SECRET, user);
    test_support::connect_pgwire_with_mapping(addr, &password, user, GRAPH, mapping)
        .await
        .expect("projection session")
}

/// Four statement kinds on the projected table, as the given principal, all refused with
/// SQLSTATE 25006 and `PROJECTION_READ_ONLY`.
async fn assert_writes_refused(addr: &str, user: &str) {
    let client = projection_client(addr, user, &format!("{MAPPING}@1")).await;
    for sql in [
        "INSERT INTO people (id, name, age) VALUES (99, 'Eve', 20)",
        "UPDATE people SET name = 'Changed' WHERE id = 10",
        "DELETE FROM people WHERE id = 10",
        "CREATE TABLE people_copy (id INT)",
    ] {
        let error = client
            .simple_query(sql)
            .await
            .expect_err(&format!("{user}: `{sql}` must be refused"));
        let db = error.as_db_error().expect("a database error");
        assert_eq!(db.code().code(), "25006", "{user}: {sql}");
        assert!(
            db.message().starts_with("PROJECTION_READ_ONLY"),
            "{user}: {sql}: {}",
            db.message()
        );
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.4.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projected_writes_refuse_for_both_principals() {
    let state = state();
    seed(&state).await;
    let before = answer(&read(&state, 10, request_for(MAPPING, 1)).await);
    let addr = listener(&state).await;

    // The read-only principal and the principal that holds a separate graph write grant.
    assert_writes_refused(&addr, READER).await;
    assert_writes_refused(&addr, MUTATOR).await;

    // The graph is unchanged: same revision, same rows, same digest.
    let after = answer(&read(&state, 11, request_for(MAPPING, 1)).await);
    assert_eq!(after.graph_revision, before.graph_revision);
    assert_eq!(after.result_digest, before.result_digest);
    assert_eq!(after.rows, before.rows);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.4.3
#[tokio::test]
async fn authorized_mutation_is_visible_in_next_read() {
    let state = state();
    seed(&state).await;
    let before = answer(&read(&state, 10, request_for(MAPPING, 1)).await);

    // An unauthorized mutation refuses with the gateway's ACCESS_DENIED and changes nothing.
    let denied = test_support::dispatch(
        &state,
        common::signed_request_with_scopes(
            SECRET,
            11,
            GRAPH,
            &["kg:read"],
            Method::AddNode {
                node_id: "person-50".to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&person(50, "Mallory", 40)).unwrap(),
            },
        ),
    )
    .await;
    refused(&denied, "ACCESS_DENIED");
    let unchanged = answer(&read(&state, 12, request_for(MAPPING, 1)).await);
    assert_eq!(unchanged.graph_revision, before.graph_revision);
    assert_eq!(unchanged.result_digest, before.result_digest);

    // An authorized mutation commits through the gateway; the next read reflects it.
    put_person(&state, 13, 50, "Margaret", 38).await;
    let after = answer(&read(&state, 14, request_for(MAPPING, 1)).await);
    assert!(
        after.graph_revision > before.graph_revision,
        "{after:?} vs {before:?}"
    );
    assert_ne!(after.result_digest, before.result_digest);
    assert_eq!(after.rows.len(), before.rows.len() + 1);
    assert!(after.rows.iter().any(|r| r["name"] == json!("Margaret")));
    assert_eq!(after.mapping_revision, 1);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.4
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projected_table_read_binds_revisions_and_refuses_writes() {
    let state = state();
    seed(&state).await;
    let addr = listener(&state).await;

    let first = answer(&read(&state, 10, request_for(MAPPING, 1)).await);
    assert_eq!(first.mapping_revision, 1);
    assert_eq!(first.result_digest, result_digest(&first.rows));

    for user in [READER, MUTATOR] {
        assert_writes_refused(&addr, user).await;
    }
    let after_refusals = answer(&read(&state, 11, request_for(MAPPING, 1)).await);
    assert_eq!(after_refusals.graph_revision, first.graph_revision);
    assert_eq!(after_refusals.result_digest, first.result_digest);

    put_person(&state, 12, 60, "Barbara", 51).await;
    let after_mutation = answer(&read(&state, 13, request_for(MAPPING, 1)).await);
    assert!(after_mutation.graph_revision > first.graph_revision);
    assert_eq!(after_mutation.rows.len(), first.rows.len() + 1);

    put_mapping(&state, 14, "draft", 1, "proposed").await;
    refused(
        &read(&state, 15, request_for("draft", 1)).await,
        "PROJECTION_MAPPING_UNAPPROVED",
    );
}
