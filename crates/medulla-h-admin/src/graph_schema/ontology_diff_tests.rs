//! Ontology diff and impact (MEDULLA-TYPED-PACKS-R804.2).

use super::ontology_diff::{diff_axioms, impact, ImpactSnapshot, SchemaSide};
use super::ontology_diff_serve::{change_requests_of, diff_ontology};
use super::ontology_import::{import_ontology, OntologyAuthority};
use medulla_catalog_types::graph_schema::{DiffTarget, GraphSchemaOp};
use medulla_graph_core::graph::GraphCore;
use medulla_types::contract::{Digest256, ResourceId};
use medulla_types::ontology_diff::ChangeImpact;

const PREFIXES: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n@prefix ex: <http://example.org/onto#> .\n";

fn side(revision: &str, ontology: &str, shapes: Option<&str>) -> SchemaSide {
    SchemaSide {
        revision: revision.to_string(),
        ontology_ttl: format!("{PREFIXES}{ontology}"),
        shapes_ttl: shapes.map(|s| format!("{PREFIXES}{s}")),
    }
}

fn graph(data: &str) -> medulla_rdf::oxrdf::Graph {
    medulla_shacl::graph_from_turtle(&format!("{PREFIXES}{data}")).unwrap()
}

#[test]
// spec: MEDULLA-TYPED-PACKS-R804.2
fn diff_lists_added_removed_changed_and_deprecated() {
    let from = side(
        "1",
        "ex:Alpha a owl:Class . ex:Beta a owl:Class . ex:Gamma a owl:Class .",
        None,
    );
    let to = side(
        "2",
        "ex:Alpha a owl:Class . ex:Gamma a owl:Class, owl:DeprecatedClass ; owl:deprecated true . ex:Delta a owl:Class .",
        None,
    );
    let diff = diff_axioms(&from, &to).unwrap();
    assert_eq!(diff.from_rev, "1");
    assert_eq!(diff.to_rev, "2");
    assert_eq!(diff.added, vec!["http://example.org/onto#Delta"]);
    assert_eq!(diff.removed, vec!["http://example.org/onto#Beta"]);
    assert_eq!(diff.changed, vec!["http://example.org/onto#Gamma"]);
    assert_eq!(diff.deprecated, vec!["http://example.org/onto#Gamma"]);
    assert_eq!(
        diff_axioms(&from, &from).unwrap().changed,
        Vec::<String>::new()
    );
}

#[test]
// spec: MEDULLA-TYPED-PACKS-R804.2
fn removed_class_reports_impact() {
    let ontology_from = "ex:Alpha a owl:Class . ex:Beta a owl:Class .";
    let from = side(
        "1",
        ontology_from,
        Some("ex:AlphaShape a sh:NodeShape ; sh:targetClass ex:Alpha ; sh:property [ sh:path ex:name ; sh:minCount 1 ] ."),
    );
    // Revision 2 removes Beta, keeps Alpha, and tightens the Alpha shape so a
    // node that passed before now fails; the shape also names the removed Beta.
    let to = side(
        "2",
        "ex:Alpha a owl:Class .",
        Some("ex:AlphaShape a sh:NodeShape ; sh:targetClass ex:Alpha ; sh:property [ sh:path ex:name ; sh:minCount 1 ] , [ sh:path ex:code ; sh:minCount 1 ] .\nex:BetaShape a sh:NodeShape ; sh:targetClass ex:Beta ."),
    );
    let snapshot = ImpactSnapshot {
        data: graph(
            "ex:a1 a ex:Alpha ; ex:name \"one\" ; ex:code \"c\" .\nex:a2 a ex:Alpha ; ex:name \"two\" .",
        ),
        mappings: vec![
            (
                "map-beta".into(),
                format!("{PREFIXES}<urn:m1> <http://www.w3.org/ns/r2rml#class> ex:Beta ."),
            ),
            (
                "map-alpha".into(),
                format!("{PREFIXES}<urn:m2> <http://www.w3.org/ns/r2rml#class> ex:Alpha ."),
            ),
        ],
        queries: vec![
            (
                "uses-beta".into(),
                "SELECT ?s WHERE { ?s a <http://example.org/onto#Beta> }".into(),
            ),
            (
                "uses-alpha".into(),
                "SELECT ?s WHERE { ?s a <http://example.org/onto#Alpha> }".into(),
            ),
        ],
        compose: None,
    };

    let (counts, items) = impact(&from, &to, &snapshot).unwrap();
    assert_eq!(items.instances, vec!["http://example.org/onto#a2"]);
    assert_eq!(items.shapes, vec!["http://example.org/onto#BetaShape"]);
    assert_eq!(items.mappings, vec!["map-beta"]);
    assert_eq!(items.queries, vec!["uses-beta"]);
    assert_eq!(counts.instances_invalid, 1);
    assert_eq!(counts.shapes_broken, 1);
    assert_eq!(counts.mappings_broken, 1);
    assert_eq!(counts.queries_broken, 1);

    // Nothing removed and nothing tightened: no impact.
    let (none, empty) = impact(&from, &from, &snapshot).unwrap();
    assert_eq!(
        none.instances_invalid + none.shapes_broken + none.mappings_broken + none.queries_broken,
        0
    );
    assert_eq!(empty, Default::default());
}

const IRI: &str = "http://example.org/onto";

fn authority(importer: &str, tenant: &str) -> OntologyAuthority {
    OntologyAuthority {
        importer: importer.to_string(),
        tenant: tenant.to_string(),
        at_ms: 1,
    }
}

fn import_op(ontology: &str, shapes: Option<&str>) -> GraphSchemaOp {
    let ontology_ttl = format!("{PREFIXES}{ontology}");
    GraphSchemaOp::ImportOntology {
        connector: ResourceId::new("weather").unwrap(),
        ontology_iri: IRI.to_string(),
        version: "1".to_string(),
        content_digest: Digest256::sha256(ontology_ttl.as_bytes()),
        ontology_ttl,
        shapes_ttl: shapes.map(|shapes| format!("{PREFIXES}{shapes}")),
        expected_prior_revision: None,
    }
}

fn diff_op(from_revision: u64, to: DiffTarget) -> GraphSchemaOp {
    GraphSchemaOp::DiffOntology {
        connector: ResourceId::new("weather").unwrap(),
        ontology_iri: IRI.to_string(),
        from_revision,
        to,
    }
}

/// A stored query node as `StoredQueryPut` writes it.
fn put_stored_query(core: &GraphCore, name: &str, text: &str) {
    let node = serde_json::json!({
        "type": "StoredQuery",
        "name": name,
        "version": 1,
        "query": {"name": name, "version": 1, "text": text},
    });
    core.add_node(
        format!("stored_query:{name}:1"),
        rmp_serde::to_vec_named(&node).unwrap(),
    );
}

fn history_impact(core: &GraphCore, revision: usize) -> Option<ChangeImpact> {
    let sources = core.schema_sources();
    let source = sources
        .dynamic
        .values()
        .find(|source| {
            matches!(
                source.origin,
                medulla_graph_core::graph::SchemaSourceOrigin::Ontology { .. }
            )
        })
        .unwrap();
    match &source.origin {
        medulla_graph_core::graph::SchemaSourceOrigin::Ontology { history, .. } => {
            history[revision].change_impact.clone()
        }
        _ => unreachable!(),
    }
}

// spec: MEDULLA-TYPED-PACKS-R804.3
#[test]
fn import_reports_impact_and_opens_change_request() {
    let core = GraphCore::new();
    let alice = authority("alice", "tenant-a");
    let shapes_one = "ex:AlphaShape a sh:NodeShape ; sh:targetClass ex:Alpha .";
    import_ontology(
        &core,
        "g",
        &import_op(
            "ex:Alpha a owl:Class . ex:Beta a owl:Class .",
            Some(shapes_one),
        ),
        &alice,
    )
    .unwrap();
    assert!(
        change_requests_of(&core, "tenant-a").is_empty(),
        "a first import breaks nothing"
    );
    put_stored_query(
        &core,
        "uses-beta",
        "SELECT ?s WHERE { ?s a <http://example.org/onto#Beta> }",
    );

    // Revision 2 removes Beta: the stored query that names it breaks.
    import_ontology(
        &core,
        "g",
        &import_op("ex:Alpha a owl:Class .", Some(shapes_one)),
        &alice,
    )
    .unwrap();
    assert_eq!(
        history_impact(&core, 1),
        Some(ChangeImpact {
            instances_invalid: 0,
            shapes_broken: 0,
            mappings_broken: 0,
            queries_broken: 1,
        })
    );
    let requests = change_requests_of(&core, "tenant-a");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["kind"], "ontology.breaking_change");
    assert_eq!(requests[0]["state"], "open");
    assert_eq!(requests[0]["reviewers"], serde_json::json!(["alice"]));
    assert_eq!(requests[0]["revision"], 2);
    assert_eq!(requests[0]["impact"]["queries_broken"], 1);

    // A later import that removes nothing opens no further request.
    import_ontology(
        &core,
        "g",
        &import_op(
            "ex:Alpha a owl:Class . ex:Gamma a owl:Class .",
            Some(shapes_one),
        ),
        &alice,
    )
    .unwrap();
    assert_eq!(history_impact(&core, 2).map(|c| c.queries_broken), Some(0));
    assert_eq!(change_requests_of(&core, "tenant-a").len(), 1);

    // The served read reports the impact of a candidate and writes nothing.
    put_stored_query(
        &core,
        "uses-gamma",
        "SELECT ?s WHERE { ?s a <http://example.org/onto#Gamma> }",
    );
    let before = core.schema_sources();
    let served = diff_ontology(
        &core,
        "g",
        "tenant-a",
        &diff_op(
            3,
            DiffTarget::Candidate {
                turtle: format!("{PREFIXES}ex:Alpha a owl:Class . ex:Beta a owl:Class ."),
            },
        ),
    )
    .unwrap();
    let report = served.ontology_diff.expect("a diff report");
    assert_eq!(report.diff.removed, vec!["http://example.org/onto#Gamma"]);
    assert_eq!(report.diff.added, vec!["http://example.org/onto#Beta"]);
    assert_eq!(report.items.queries, vec!["uses-gamma"]);
    assert_eq!(report.impact.queries_broken, 1);
    assert!(!served.changed);
    assert_eq!(
        core.schema_sources().composed_digest(),
        before.composed_digest()
    );
    assert_eq!(change_requests_of(&core, "tenant-a").len(), 1);

    // Cross-tenant negatives: another tenant sees no request, an invalid
    // tenant is refused, and another principal cannot revise the ontology.
    assert!(change_requests_of(&core, "tenant-b").is_empty());
    let denied = diff_ontology(&core, "g", "", &diff_op(3, DiffTarget::Revision(3))).unwrap_err();
    assert!(denied.starts_with("TENANT_DENIED"), "{denied}");
    let mallory = import_ontology(
        &core,
        "g",
        &import_op("ex:Alpha a owl:Class .", None),
        &authority("mallory", "tenant-b"),
    )
    .unwrap_err();
    assert!(mallory.starts_with("IMPORTER_MISMATCH"), "{mallory}");
    assert!(change_requests_of(&core, "tenant-b").is_empty());
}

// spec: MEDULLA-TYPED-PACKS-R804.5
#[test]
fn impact_names_items() {
    let core = GraphCore::new();
    let classes = |count: usize| {
        (0..count)
            .map(|index| format!("ex:C{index:03} a owl:Class ."))
            .collect::<Vec<_>>()
            .join("\n")
    };
    import_ontology(
        &core,
        "g",
        &import_op(&classes(200), None),
        &authority("alice", "tenant-a"),
    )
    .unwrap();
    for index in 50..200 {
        put_stored_query(
            &core,
            &format!("q{index:03}"),
            &format!("SELECT ?s WHERE {{ ?s a <http://example.org/onto#C{index:03}> }}"),
        );
    }
    // Remove 150 classes: every one of the 150 queries breaks.
    let served = diff_ontology(
        &core,
        "g",
        "tenant-a",
        &diff_op(
            1,
            DiffTarget::Candidate {
                turtle: format!("{PREFIXES}{}", classes(50)),
            },
        ),
    )
    .unwrap();
    let report = served.ontology_diff.unwrap();
    assert_eq!(report.diff.removed.len(), 150);
    assert_eq!(report.impact.queries_broken, 150, "counts stay exact");
    assert_eq!(report.items.queries.len(), 100, "ids are cut to the knob");
    let mut sorted = report.items.queries.clone();
    sorted.sort();
    assert_eq!(report.items.queries, sorted);
    assert_eq!(report.items.queries[0], "q050");
}
