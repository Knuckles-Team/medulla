//! Lifecycle vocabulary contract (MEDULLA-SDLC-R001.1): the closed edge set,
//! one SHACL shape per edge with its domain and range, and the
//! `LifecycleEdgeKind` mirror. The ontology and the shapes are read back from
//! the registered `sdd` core source, so a drift between the wire vocabulary,
//! the ontology document and the shapes fails here, not at ingest time.

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};
use medulla_shacl::validate_turtle;
use medulla_source_types::lifecycle::LifecycleEdgeKind;

const KG: &str = "http://knuckles.team/kg#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const SHACL: &str = "http://www.w3.org/ns/shacl#";
const SHACL_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";

/// (edge, domain class, range class) — the closed set of plan.md, one row per
/// `LifecycleEdgeKind` in declaration order.
const EDGES: [(LifecycleEdgeKind, &str, &str); 14] = [
    (LifecycleEdgeKind::Changes, "Changeset", "CodeSymbol"),
    (
        LifecycleEdgeKind::CitesRequirement,
        "Changeset",
        "Requirement",
    ),
    (LifecycleEdgeKind::Implements, "CodeSymbol", "Requirement"),
    (LifecycleEdgeKind::VerifiedBy, "Requirement", "TestCase"),
    (LifecycleEdgeKind::Exercises, "TestCase", "CodeSymbol"),
    (LifecycleEdgeKind::Judges, "GateVerdict", "Changeset"),
    (LifecycleEdgeKind::ProducedBy, "Changeset", "RunTrace"),
    (LifecycleEdgeKind::Affects, "Defect", "CodeSymbol"),
    (LifecycleEdgeKind::IntroducedBy, "Defect", "Changeset"),
    (LifecycleEdgeKind::FixedBy, "Defect", "Changeset"),
    (LifecycleEdgeKind::Contains, "Release", "Changeset"),
    (LifecycleEdgeKind::Deploys, "Deployment", "Release"),
    (LifecycleEdgeKind::ObservedIn, "Incident", "Deployment"),
    (LifecycleEdgeKind::Raises, "Incident", "Defect"),
];

fn kg(local: &str) -> String {
    format!("{KG}{local}")
}

fn subject_is(triple: &Triple, subject: &str) -> bool {
    match &triple.subject {
        NamedOrBlankNode::NamedNode(node) => node.as_str() == subject,
        NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()) == subject,
    }
}

fn predicate_is(triple: &Triple, predicate: &str) -> bool {
    triple.predicate.as_str() == predicate
}

/// Object values of `subject predicate ?object` as strings (IRIs and literals).
fn object_values(triples: &[Triple], subject: &str, predicate: &str) -> Vec<String> {
    triples
        .iter()
        .filter(|triple| subject_is(triple, subject) && predicate_is(triple, predicate))
        .filter_map(|triple| match &triple.object {
            Term::NamedNode(node) => Some(node.as_str().to_string()),
            Term::Literal(literal) => Some(literal.value().to_string()),
            Term::BlankNode(node) => Some(format!("_:{}", node.as_str())),
            _ => None,
        })
        .collect()
}

/// The subjects typed as `sh:NodeShape`.
fn node_shapes(shapes: &[Triple]) -> Vec<String> {
    shapes
        .iter()
        .filter(|triple| {
            predicate_is(triple, RDF_TYPE)
                && matches!(
                    &triple.object,
                    Term::NamedNode(node) if node.as_str() == SHACL_NODE_SHAPE
                )
        })
        .map(|triple| match &triple.subject {
            NamedOrBlankNode::NamedNode(node) => node.as_str().to_string(),
            NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()),
        })
        .collect()
}

/// One fixture per edge: a fresh instance of the domain class carrying one
/// edge to a fresh instance of the range class.
fn edge_fixture(edge: &str, domain: &str, range: &str) -> String {
    format!(
        r#"
@prefix : <http://knuckles.team/kg#> .
@prefix ex: <http://example.org/lifecycle/{edge}/> .

ex:subject a :{domain} ;
    :{edge} ex:object .

ex:object a :{range} .
"#
    )
}

// spec: MEDULLA-SDLC-R001.1
#[test]
fn edge_set_is_closed_and_shaped() {
    let sources = GraphSchemaSources::default();
    sources
        .validate()
        .expect("the default core catalog validates");
    let source = sources
        .core
        .get("core:sdd@1")
        .expect("the sdd vocabulary is registered");
    let ontology_ttl = source
        .ontology_ttl
        .as_deref()
        .expect("the sdd source carries the ontology");
    let shapes_ttl = source
        .shapes_ttl
        .as_deref()
        .expect("the sdd source carries the lifecycle shapes");

    let ontology = parse_turtle(ontology_ttl).expect("the sdd ontology parses");
    let shapes = parse_turtle(shapes_ttl).expect("the sdd shapes parse");

    // The record kinds are declared and extend an engine core class
    // (bfo:0000015 and bfo:0000031 are core-foundation classes).
    for class in ["Release", "Deployment", "Defect"] {
        let iri = kg(class);
        assert!(
            object_values(&ontology, &iri, RDF_TYPE).contains(&OWL_CLASS.to_string()),
            "{class} is declared as an owl:Class"
        );
        assert!(
            !object_values(&ontology, &iri, RDFS_SUBCLASS_OF).is_empty(),
            "{class} extends an engine core class"
        );
    }

    // Closed set, first direction: every LifecycleEdgeKind is declared in the
    // ontology, never a second edge list.
    for (kind, _, _) in EDGES {
        assert!(
            object_values(&ontology, &kg(kind.name()), RDF_TYPE)
                .contains(&OWL_OBJECT_PROPERTY.to_string()),
            "{} is declared as an owl:ObjectProperty",
            kind.name()
        );
    }

    // Closed set, second direction: the NodeShapes point at exactly the
    // LifecycleEdgeKind set, one shape per edge, its domain as target and its
    // range as class. A mismatch fails the test.
    let mut shaped: BTreeSet<String> = BTreeSet::new();
    for shape in node_shapes(&shapes) {
        let target = object_values(&shapes, &shape, &format!("{SHACL}targetClass"));
        for block in object_values(&shapes, &shape, &format!("{SHACL}property")) {
            let path = object_values(&shapes, &block, &format!("{SHACL}path"));
            let class = object_values(&shapes, &block, &format!("{SHACL}class"));
            let Some(edge) = path.first() else {
                continue;
            };
            let (kind, domain, range) = EDGES
                .iter()
                .find(|(kind, _, _)| kg(kind.name()) == *edge)
                .expect("a shape points at a non-lifecycle edge");
            assert!(
                target.contains(&kg(domain)),
                "the {kind:?} shape targets its domain {domain}"
            );
            assert!(
                class.contains(&kg(range)),
                "the {kind:?} shape carries its range {range}"
            );
            shaped.insert(edge.clone());
        }
    }
    let expected: BTreeSet<String> = EDGES.iter().map(|(kind, _, _)| kg(kind.name())).collect();
    assert_eq!(
        shaped, expected,
        "the shaped edge set is the LifecycleEdgeKind set, no more and no less"
    );

    // One fixture per edge validates: the domain instance carries one edge to
    // an instance of the range class.
    let corpus = format!("{shapes_ttl}{ontology_ttl}");
    for (kind, domain, range) in EDGES {
        let fixture = edge_fixture(kind.name(), domain, range);
        let report =
            validate_turtle(&corpus, &fixture).expect("the fixture parses before validation");
        assert!(
            report.conforms,
            "the {kind:?} fixture must conform to the lifecycle shapes: {:?}",
            report.results
        );
    }

    // A foreign range is refused: the shapes are not vacuous.
    let wrong_range = r#"
@prefix : <http://knuckles.team/kg#> .
@prefix ex: <http://example.org/lifecycle/wrong-range/> .

ex:changeset a :Changeset ;
    :changes ex:requirement .

ex:requirement a :Requirement .
"#;
    let report =
        validate_turtle(&corpus, wrong_range).expect("the fixture parses before validation");
    assert!(
        !report.conforms,
        "a changes edge to a non-CodeSymbol must be refused: {:?}",
        report.results
    );
}
