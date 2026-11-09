//! SPARQL 1.1 Update forms (MEDULLA-FEDERATED-QUERY-R707.1).
//!
//! Audit result: the update parser (`medulla_rdf::update::parse_update`, over the
//! spargebra model) already accepts all ten SPARQL 1.1 Update forms. `COPY`, `MOVE`
//! and `ADD` are graph-management shorthands that the parser lowers to the existing
//! `GraphUpdateOperation` variants (`Drop` plus `DeleteInsert`), so no new operation
//! kind is needed and the existing `UpdateOp` model is reused unchanged.
//!
//! R707.2 (behavior): a multi-operation update applies as one atomic unit at the
//! engine boundary — a failure in any operation leaves the store unchanged.

#![cfg(feature = "sparql")]

use medulla_rdf::spargebra::GraphUpdateOperation;
use medulla_rdf::sparql::Projection;
use medulla_rdf::update::{execute_str, parse_update, GraphStore, MapStore, UpdateError};

struct TestWriteGuard;

impl medulla_rdf::guard::WriteGuard for TestWriteGuard {
    fn check_graph(
        &self,
        _graph: Option<&str>,
        _base: &medulla_rdf::oxrdf::Graph,
        _additions: &[medulla_rdf::oxrdf::Triple],
        _removals: &[medulla_rdf::oxrdf::Triple],
    ) -> Result<(), medulla_rdf::guard::GuardRejection> {
        Ok(())
    }
}

/// The whole-store image: every graph (the default one plus each named one) as its
/// sorted N-Triples lines, so a before/after comparison is an exact store equality.
fn store_image(store: &MapStore) -> Vec<(String, Vec<String>)> {
    let default = medulla_rdf::mapping::export_triples(&store.core_of(None), "").unwrap();
    let mut graphs = vec![(
        String::new(),
        medulla_rdf::mapping::to_ntriples(&default).unwrap(),
    )];
    graphs.extend(store.named().into_iter().map(|(name, core)| {
        let body = medulla_rdf::mapping::to_ntriples(
            &medulla_rdf::mapping::export_triples(&core, &name).unwrap(),
        )
        .unwrap();
        (name, body)
    }));
    graphs.sort_by(|a, b| a.0.cmp(&b.0));
    graphs
        .into_iter()
        .map(|(name, body)| {
            let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
            lines.sort();
            (name, lines)
        })
        .collect()
}

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

// spec: MEDULLA-FEDERATED-QUERY-R707.2
/// A multi-operation update is ONE atomic unit at the engine boundary: when the
/// third operation fails, the first two — already applied on the shadow replay —
/// must not leak into the real store. The whole-store image comparison proves no
/// partial write survived the failure.
#[test]
fn multi_operation_update_is_atomic() {
    let store = MapStore::new();
    execute_str(
        "PREFIX ex: <http://ex/>
         INSERT DATA {
           ex:a ex:keep ex:b .
           ex:a ex:drop ex:c
         }",
        &store,
        &Projection::raw(),
        &TestWriteGuard,
    )
    .unwrap();
    let before = store_image(&store);

    // Three operations in one request: op1 inserts, op2 deletes, op3 is a
    // non-SILENT remote LOAD — the engine write path refuses it.
    let failed = execute_str(
        "PREFIX ex: <http://ex/>
         INSERT DATA { ex:x ex:p ex:y } ;
         DELETE DATA { ex:a ex:drop ex:c } ;
         LOAD <http://remote.example/data.ttl>",
        &store,
        &Projection::raw(),
        &TestWriteGuard,
    );
    assert!(
        matches!(&failed, Err(UpdateError::Exec(message)) if message.contains("LOAD")),
        "the third operation must be the one that fails: {failed:?}"
    );

    assert_eq!(
        store_image(&store),
        before,
        "a failure in any operation must leave the store unchanged"
    );
}
