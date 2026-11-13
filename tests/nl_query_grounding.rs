//! Source-native identifier grounding through the served dispatch chain
//! (MEDULLA-FEDERATED-QUERY-R058.4.7.2): a natural-language plan that names a
//! decision-log column, time-series name or channel, CEP attribute, UDF id or
//! RDF/SPARQL IRI the caller's catalog does not hold is refused with
//! `NL_IDENTIFIER_UNKNOWN{family: ...}` before anything executes, and a name the
//! catalog holds grounds.
//!
//! Every test rides `Method::NlQuery` -> `dispatch_query_family` ->
//! `handle_nl_query_plan` with a planner that answers the question text as the UQL.
#![cfg(all(feature = "server", feature = "security", feature = "nl-query"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response};

const SECRET: &str = "nl-query-grounding-secret";
const GRAPH: &str = "__commons__";

/// Answers the question text as the UQL, so each test scripts its plan directly.
struct EchoPlanner;

impl medulla_nlq::nl::NlPlanner for EchoPlanner {
    fn plan(&self, nl: &str, _schema_hint: &str) -> Result<String, String> {
        Ok(nl.to_string())
    }
}

fn state() -> test_support::SharedState {
    common::configure_authority();
    medulla_h_query::nl::set_nl_planner(Arc::new(EchoPlanner));
    #[cfg(feature = "tsdb")]
    {
        let (persist_dir, persistence) = common::tempdir_persistence();
        test_support::state_with_tsdb(
            SECRET,
            common::current_isolation(),
            persist_dir,
            persistence,
            Some(test_support::temporary_series("nl-grounding")),
        )
    }
    #[cfg(not(feature = "tsdb"))]
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::commons_request(SECRET, id, method)).await
}

async fn nl(state: &test_support::SharedState, id: u64, uql: &str) -> String {
    let response = send(
        state,
        id,
        Method::NlQuery {
            text: uql.into(),
            graph: GRAPH.into(),
            history: Vec::new(),
            dialect: Default::default(),
            max_steps: None,
        },
    )
    .await;
    format!("{:?} {:?}", response.error, response.error_detail)
}

/// The refusal names the family and the invented identifier and nothing ran.
fn assert_unknown(outcome: &str, family: &str, identifier: &str) {
    assert!(
        outcome.contains(&format!("NL_IDENTIFIER_UNKNOWN{{family: {family}}}")),
        "{outcome}"
    );
    assert!(outcome.contains(identifier), "{outcome}");
}

/// The plan passed grounding; it may still fail later for a reason of execution.
fn assert_grounded(outcome: &str) {
    assert!(!outcome.contains("NL_IDENTIFIER_UNKNOWN"), "{outcome}");
    assert!(!outcome.contains("NL_SCHEMA_"), "{outcome}");
}

async fn seed_doc(state: &test_support::SharedState) {
    let responses =
        test_support::add_nodes_with_property(state, SECRET, &[("d1", "title")], "Doc", "a").await;
    assert!(responses[0].1.error.is_none(), "{:?}", responses[0].1.error);
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.1
#[tokio::test]
async fn decision_record_identifiers() {
    let state = state();
    seed_doc(&state).await;
    let valid = "DECISIONS WHERE outcome = 'acted' AND committed_at_ms >= 1";
    assert_grounded(&nl(&state, 10, valid).await);
    assert_unknown(
        &nl(&state, 11, "DECISIONS WHERE invented_column = 'acted'").await,
        "decision_record",
        "invented_column",
    );
    // Source replacement: DECISIONS after a graph scan replaces the rows, and its
    // own columns are still the decision log's.
    let after_scan = "MATCH (:Doc) |> DECISIONS WHERE outcome = 'acted'";
    assert_grounded(&nl(&state, 12, after_scan).await);
    assert_unknown(
        &nl(&state, 13, "MATCH (:Doc) |> DECISIONS WHERE title = 'a'").await,
        "decision_record",
        "title",
    );
    // Empty-input reseeding: with no incoming rows the stage reseeds from the log.
    assert_grounded(&nl(&state, 14, "DECISIONS |> LIMIT 5").await);
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.2
#[cfg(feature = "tsdb")]
#[tokio::test]
async fn timeseries_identifiers() {
    let state = state();
    let points: Vec<(i64, Vec<f64>)> =
        vec![(1_000_000_000, vec![10.0]), (2_000_000_000, vec![11.0])];
    let seeded = send(
        &state,
        1,
        Method::TsAppend {
            series_id: "cpu".into(),
            n_fields: 1,
            bucket_ns: 60_000_000_000,
            field_names: vec!["value".into()],
            points_msgpack: rmp_serde::to_vec_named(&points).unwrap(),
        },
    )
    .await;
    assert!(seeded.error.is_none(), "{:?}", seeded.error);
    assert_grounded(&nl(&state, 2, "TSSCAN ['cpu'] FROM 0 TO 3600").await);
    assert_unknown(
        &nl(&state, 3, "TSSCAN ['gpu'] FROM 0 TO 3600").await,
        "timeseries",
        "gpu",
    );
    let derived = "TSSCAN ['cpu'] FROM 0 TO 3600 |> DERIVE gt(v0, 5) AS spike |> EVENTS spike";
    assert_grounded(&nl(&state, 4, derived).await);
    assert_grounded(&nl(&state, 5, "TSSCAN ['cpu'] FROM 0 TO 3600 |> EVENTS v0").await);
    assert_unknown(
        &nl(&state, 6, "TSSCAN ['cpu'] FROM 0 TO 3600 |> EVENTS v7").await,
        "timeseries",
        "v7",
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.3
#[cfg(feature = "stream")]
#[tokio::test]
async fn cep_identifiers() {
    let state = state();
    let event = test_support::json_bytes(serde_json::json!({
        "type": "Event", "ts": 1, "key": "trade", "attrs": {"qty": 500}
    }));
    let added = send(
        &state,
        1,
        Method::AddNode {
            node_id: "e1".into(),
            properties_msgpack: event,
        },
    )
    .await;
    assert!(added.error.is_none(), "{:?}", added.error);
    let cep = |attribute: &str| {
        format!(
            "MATCH (:Event) |> CEP SEQ ({{KEY 'trade' WHERE {attribute} > 100}}) WINDOW SLIDING 60"
        )
    };
    assert_grounded(&nl(&state, 2, &cep("qty")).await);
    assert_unknown(&nl(&state, 3, &cep("invented")).await, "cep", "invented");
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.4
#[cfg(feature = "wasm-udf")]
#[tokio::test]
async fn udf_identifiers() {
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::carrier::CarrierAuthority;

    const IDENTITY_WAT: &str = r#"(module
        (memory (export "memory") 1)
        (global $n (mut i32) (i32.const 1024))
        (func (export "alloc") (param $l i32) (result i32)
            (local $p i32) (local.set $p (global.get $n))
            (global.set $n (i32.add (global.get $n) (local.get $l))) (local.get $p))
        (func (export "udf") (param $p i32) (param $l i32) (result i64)
            (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32))
                    (i64.extend_i32_u (local.get $l)))))"#;
    let state = state();
    seed_doc(&state).await;
    let registered = send(
        &state,
        1,
        Method::RegisterUdf {
            id: "score-v2".into(),
            wasm: wat::parse_str(IDENTITY_WAT).expect("valid wat"),
        },
    )
    .await;
    assert!(registered.error.is_none(), "{:?}", registered.error);
    // Another tenant's UDF resolves like an unregistered id.
    let foreign = CarrierAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test_in_tenant("agent-x", "other-tenant"),
    )
    .expect("verified carrier");
    state
        .read()
        .await
        .udf_registry
        .register(
            &foreign,
            "foreign-udf",
            &wat::parse_str(IDENTITY_WAT).expect("valid wat"),
            medulla_wasm::UdfLimits::default(),
        )
        .expect("register foreign udf");
    assert_grounded(&nl(&state, 2, "MATCH (:Doc) |> UDF 'score-v2'").await);
    assert_unknown(
        &nl(&state, 3, "MATCH (:Doc) |> UDF 'invented-udf'").await,
        "udf",
        "invented-udf",
    );
    assert_unknown(
        &nl(&state, 4, "MATCH (:Doc) |> UDF 'foreign-udf'").await,
        "udf",
        "foreign-udf",
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.5
#[cfg(all(feature = "owl-plan", feature = "shacl"))]
#[tokio::test]
async fn rdf_identifiers() {
    let sources = medulla_graph_core::graph::GraphSchemaSources::default();
    let vocabulary = medulla_h_sparql::rdf::composed_vocabulary(&sources)
        .expect("the core catalog composes a vocabulary");
    let known = vocabulary.terms.iter().next().expect("a core term").clone();
    let namespace = medulla_nlq::nl::ground_sources::namespace_of(&known).expect("namespace");
    let state = state();
    let sparql = |iri: &str| format!("SPARQL 'SELECT ?x WHERE {{ ?x a <{iri}> }}' VAR 'x'");
    assert_grounded(&nl(&state, 1, &sparql(&known)).await);
    let invented = format!("{namespace}InventedClassName");
    assert_unknown(
        &nl(&state, 2, &sparql(&invented)).await,
        "rdf",
        "InventedClassName",
    );
    let by_prefix = format!(
        "SPARQL 'PREFIX p: <{namespace}> SELECT ?x WHERE {{ ?x a p:InventedClassName }}' VAR 'x'"
    );
    assert_unknown(&nl(&state, 3, &by_prefix).await, "rdf", "InventedClassName");
}
