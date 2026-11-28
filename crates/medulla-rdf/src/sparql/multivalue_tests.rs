//! EH-583 — a triple pattern over a multi-valued literal property binds EVERY value.
//!
//! Loading a subject with two values of one predicate keeps the first at the ordinary
//! property key and parks the rest in the reserved multivalue cell. The evaluator used to
//! read only the ordinary key, so the second value never bound and a FILTER on it could
//! not see it. These run on a plain (non-federated) graph.

use super::{execute, Dataset, Projection};

const DATA: &str = r#"
@prefix ex: <http://example.org/> .
ex:p ex:age 40, 18 ;
     ex:name "Alice", "Alys" .
ex:q ex:age 35 .
"#;

/// Sorted bindings of `var` for `query` (the `ex:` prefix is supplied).
fn column(query: &str, var: &str) -> Vec<String> {
    let view = super::view_of_turtle(DATA);
    let text = format!("PREFIX ex: <http://example.org/>\n{query}");
    let table = execute(
        &Dataset::new(&view, Vec::new()),
        &text,
        &Projection::raw(),
        None,
    )
    .unwrap()
    .into_table();
    let mut out: Vec<String> = table
        .solutions
        .iter()
        .filter_map(|s| s.get(var).map(|b| b.display_text().to_string()))
        .collect();
    out.sort();
    out
}

// spec: MEDULLA-FEDERATED-QUERY-R055
#[test]
fn both_values_of_one_property_bind() {
    assert_eq!(
        column("SELECT ?n WHERE { ex:p ex:name ?n }", "n"),
        vec!["Alice", "Alys"]
    );
    assert_eq!(
        column("SELECT ?a WHERE { ex:p ex:age ?a }", "a"),
        vec!["18", "40"]
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R055
#[test]
fn a_filter_applies_to_each_binding() {
    let over = column("SELECT ?a WHERE { ?s ex:age ?a FILTER (?a > 30) }", "a");
    assert_eq!(over, vec!["35", "40"]);
    let under = column("SELECT ?a WHERE { ?s ex:age ?a FILTER (?a < 30) }", "a");
    assert_eq!(under, vec!["18"], "the second value is filtered on its own");
}

// spec: MEDULLA-FEDERATED-QUERY-R055
#[test]
fn two_multi_valued_properties_join_as_a_cross_product() {
    let names = column("SELECT ?n WHERE { ex:p ex:name ?n ; ex:age ?a }", "n");
    assert_eq!(names, vec!["Alice", "Alice", "Alys", "Alys"]);
}

// spec: MEDULLA-FEDERATED-QUERY-R055
#[test]
fn a_constant_object_matches_a_second_value() {
    let subjects = column("SELECT ?s WHERE { ?s ex:name \"Alys\" }", "s");
    assert_eq!(subjects.len(), 1, "{subjects:?}");
}

/// A native node whose `machineKey` is stored as a list, beside a scalar property, a
/// typed element and elements that carry no term (a nested list and a null).
fn list_property_view() -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let mut txn = core.txn();
    txn.add_node(
        "m1".into(),
        rmp_serde::to_vec_named(&serde_json::json!({
            "type": "machine",
            "label": "rack-one",
            "machineKey": [
                "mac:aa",
                "serial:bb",
                {"value": "7", "datatype": "http://www.w3.org/2001/XMLSchema#integer"},
                ["nested"],
                null
            ],
            "emptyList": []
        }))
        .unwrap(),
    );
    drop(txn);
    core.analysis_snapshot()
}

fn list_column(query: &str, var: &str) -> Vec<String> {
    let view = list_property_view();
    let text = format!("PREFIX ex: <http://example.org/>\n{query}");
    let mut out: Vec<String> = execute(
        &Dataset::new(&view, Vec::new()),
        &text,
        &Projection::from_wire("http://example.org/", "camel"),
        None,
    )
    .unwrap()
    .into_table()
    .solutions
    .iter()
    .filter_map(|s| s.get(var).map(|b| b.display_text().to_string()))
    .collect();
    out.sort();
    out
}

// spec: MEDULLA-FEDERATED-QUERY-R055
#[test]
fn a_list_valued_node_property_projects_one_triple_per_element() {
    let by_predicate = list_column("SELECT ?k WHERE { ?s ex:machineKey ?k }", "k");
    assert_eq!(by_predicate, vec!["7", "mac:aa", "serial:bb"]);

    let by_wildcard = list_column(
        "SELECT ?k WHERE { ?s ?p ?k FILTER (?p = ex:machineKey) }",
        "k",
    );
    assert_eq!(by_wildcard, by_predicate, "the full ?s ?p ?o scan agrees");

    let by_value = list_column("SELECT ?s WHERE { ?s ex:machineKey \"serial:bb\" }", "s");
    assert_eq!(
        by_value,
        vec!["<http://example.org/m1>"],
        "one element matches by value"
    );

    let scalar = list_column("SELECT ?l WHERE { ?s ex:label ?l }", "l");
    assert_eq!(scalar, vec!["rack-one"], "a scalar property is unchanged");

    let empty = list_column("SELECT ?e WHERE { ?s ex:emptyList ?e }", "e");
    assert!(
        empty.is_empty(),
        "an empty list projects no triple: {empty:?}"
    );
}
