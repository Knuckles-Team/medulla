//! SPARQL 1.1 Update forms (MEDULLA-FEDERATED-QUERY-R707.1).
//!
//! Audit result: the update parser (`medulla_rdf::update::parse_update`, over the
//! spargebra model) already accepts all ten SPARQL 1.1 Update forms. `COPY`, `MOVE`
//! and `ADD` are graph-management shorthands that the parser lowers to the existing
//! `GraphUpdateOperation` variants (`Drop` plus `DeleteInsert`), so no new operation
//! kind is needed and the existing `UpdateOp` model is reused unchanged.

#![cfg(feature = "sparql")]

use medulla_rdf::spargebra::GraphUpdateOperation;
use medulla_rdf::update::parse_update;

fn kinds(update: &str) -> Vec<&'static str> {
    let parsed = parse_update(update).unwrap_or_else(|error| panic!("{update}: {error}"));
    parsed
        .operations
        .iter()
        .map(|op| match op {
            GraphUpdateOperation::InsertData { .. } => "insert_data",
            GraphUpdateOperation::DeleteData { .. } => "delete_data",
            GraphUpdateOperation::DeleteInsert { .. } => "delete_insert",
            GraphUpdateOperation::Load { .. } => "load",
            GraphUpdateOperation::Clear { .. } => "clear",
            GraphUpdateOperation::Create { .. } => "create",
            GraphUpdateOperation::Drop { .. } => "drop",
        })
        .collect()
}

// spec: MEDULLA-FEDERATED-QUERY-R707.1
#[test]
fn update_forms_parse() {
    let cases: [(&str, &str); 10] = [
        (
            "INSERT DATA",
            "INSERT DATA { <urn:a> <urn:p> <urn:b> . GRAPH <urn:g> { <urn:a> <urn:p> \"x\" } }",
        ),
        ("DELETE DATA", "DELETE DATA { <urn:a> <urn:p> <urn:b> }"),
        (
            "DELETE/INSERT WHERE",
            "DELETE { ?s <urn:p> ?o } INSERT { ?s <urn:q> ?o } WHERE { ?s <urn:p> ?o }",
        ),
        (
            "LOAD",
            "LOAD SILENT <http://example.org/data.ttl> INTO GRAPH <urn:g>",
        ),
        ("CLEAR", "CLEAR GRAPH <urn:g>"),
        ("CREATE", "CREATE SILENT GRAPH <urn:g>"),
        ("DROP", "DROP ALL"),
        ("COPY", "COPY <urn:g1> TO <urn:g2>"),
        ("MOVE", "MOVE <urn:g1> TO <urn:g2>"),
        ("ADD", "ADD <urn:g1> TO <urn:g2>"),
    ];
    for (form, text) in cases {
        let parsed = kinds(text);
        assert!(!parsed.is_empty(), "{form} parsed to no operation");
        let expected: &[&str] = match form {
            "INSERT DATA" => &["insert_data"],
            "DELETE DATA" => &["delete_data"],
            "DELETE/INSERT WHERE" => &["delete_insert"],
            "LOAD" => &["load"],
            "CLEAR" => &["clear"],
            "CREATE" => &["create"],
            "DROP" => &["drop"],
            // COPY replaces the target: drop it, then copy every triple across.
            "COPY" => &["drop", "delete_insert"],
            // MOVE is COPY followed by dropping the source.
            "MOVE" => &["drop", "delete_insert", "drop"],
            // ADD merges: copy every triple across, keeping the target.
            "ADD" => &["delete_insert"],
            _ => unreachable!(),
        };
        assert_eq!(parsed, expected, "{form} lowered unexpectedly");
    }
    // A multi-operation request keeps every clause in order.
    assert_eq!(
        kinds(
            "INSERT DATA { <urn:a> <urn:p> <urn:b> } ; CLEAR DEFAULT ; DROP SILENT GRAPH <urn:g>"
        ),
        ["insert_data", "clear", "drop"]
    );
}
