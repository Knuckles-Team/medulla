//! Served `ShexValidate` through the real `dispatch` (MEDULLA-FEDERATED-QUERY-R084.x).
//!
//! Each test sends an authenticated `Method::ShexValidate` request and reads the
//! `ShexValidationReport` the handler returns.
#![cfg(all(feature = "server", feature = "shex"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};

const SECRET: &str = "shex-served-secret";

const PREFIXES: &str = "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
    @prefix ex: <http://example.org/> .\n";

/// A `PersonShape` requiring exactly one `ex:name` of datatype `xsd:string`.
const PERSON_SCHEMA: &str = r#"{
  "type": "Schema",
  "shapes": [
    { "type": "Shape",
      "id": "http://example.org/PersonShape",
      "expression": {
        "type": "TripleConstraint",
        "predicate": "http://example.org/name",
        "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" },
        "min": 1, "max": 1
      } }
  ]
}"#;

const SHAPE: &str = "http://example.org/PersonShape";

fn state() -> test_support::SharedState {
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn validate(data: &str, pairs: &[(&str, &str)]) -> Response {
    let state = state();
    test_support::dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::ShexValidate {
                schema: PERSON_SCHEMA.to_string(),
                data_graph: format!("{PREFIXES}{data}"),
                shape_map: pairs
                    .iter()
                    .map(|(node, shape)| [node.to_string(), shape.to_string()])
                    .collect(),
            },
        ),
    )
    .await
}

fn report(response: &Response) -> serde_json::Value {
    assert!(response.error.is_none(), "{response:?}");
    match &response.result {
        Some(ResultPayload::Json(value)) => value.clone(),
        other => panic!("expected a Json ShEx report, got {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R084.1
#[tokio::test]
async fn wire_report_preserves_shape_map() {
    let data = r#"ex:alice ex:name "Alice" . ex:bob ex:age "1" . ex:carol ex:name "Carol" ."#;
    let pairs = [
        ("http://example.org/carol", SHAPE),
        ("http://example.org/bob", SHAPE),
        ("http://example.org/alice", SHAPE),
    ];
    let body = report(&validate(data, &pairs).await);
    let results = body["results"].as_array().expect("results");
    let nodes: Vec<&str> = results
        .iter()
        .map(|result| result["node"].as_str().expect("node"))
        .collect();
    assert_eq!(
        nodes,
        pairs.iter().map(|(node, _)| *node).collect::<Vec<_>>(),
        "results keep shape-map order and node lexical form"
    );
    for result in results {
        assert_eq!(result["shape"], SHAPE);
    }
    let conforms: Vec<bool> = results
        .iter()
        .map(|result| result["conforms"].as_bool().expect("conforms"))
        .collect();
    assert_eq!(conforms, [true, false, true]);
    assert_eq!(body["conforms"], false, "one failure fails the aggregate");

    let empty = report(&validate(data, &[]).await);
    assert_eq!(empty["conforms"], true, "an empty map is a true aggregate");
    assert_eq!(empty["results"].as_array().map(Vec::len), Some(0));
}

// spec: MEDULLA-FEDERATED-QUERY-R084.3
#[tokio::test]
async fn bounded_constraints_report_results() {
    let ok = report(
        &validate(
            r#"ex:alice ex:name "Alice" ."#,
            &[("http://example.org/alice", SHAPE)],
        )
        .await,
    );
    assert_eq!(ok["conforms"], true);
    assert!(ok["results"][0]["reason"].is_null());

    // Two names exceed `max: 1`.
    let bad = report(
        &validate(
            r#"ex:dan ex:name "Dan" , "Daniel" ."#,
            &[("http://example.org/dan", SHAPE)],
        )
        .await,
    );
    assert_eq!(bad["conforms"], false);
    let reason = bad["results"][0]["reason"].as_str().expect("a reason");
    assert!(reason.contains("http://example.org/name"), "{reason}");
}

// spec: MEDULLA-FEDERATED-QUERY-R084.5
#[tokio::test]
async fn real_dispatch_reaches_shex_validator() {
    let data = r#"ex:bob ex:age "1" ."#;
    let pairs = [("http://example.org/bob", SHAPE)];
    let served = report(&validate(data, &pairs).await);
    let direct = medulla_shex::validate_shexj(PERSON_SCHEMA, &format!("{PREFIXES}{data}"), &pairs)
        .expect("direct validation");
    assert_eq!(served["conforms"], direct.conforms);
    let served_results = served["results"].as_array().expect("results");
    assert_eq!(served_results.len(), direct.results.len());
    for (served, direct) in served_results.iter().zip(&direct.results) {
        assert_eq!(served["node"], direct.node.as_str());
        assert_eq!(served["shape"], direct.shape.as_str());
        assert_eq!(served["conforms"], direct.conforms);
        assert_eq!(served["reason"].as_str(), direct.reason.as_deref());
    }
}
