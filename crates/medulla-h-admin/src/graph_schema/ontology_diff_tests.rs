//! Ontology diff and impact (MEDULLA-TYPED-PACKS-R804.2).

use super::ontology_diff::{diff_axioms, impact, ImpactSnapshot, SchemaSide};

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
