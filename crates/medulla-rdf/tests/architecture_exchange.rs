//! ArchiMate exchange mapping annotations (MEDULLA-CQ-ORCH-R705.1): every
//! mapped class and edge resolves exactly one ArchiMate type from the ontology.
#![cfg(all(feature = "owl", feature = "sparql"))]

use medulla_rdf::mapping::parse_turtle;
use medulla_shacl::validate_turtle;

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const ONTOLOGY: &[&str] = &[
    include_str!("../../medulla-core/ontology/archimate-v1.ttl"),
    include_str!("../../medulla-core/ontology/orchestration-v1.ttl"),
    include_str!("../../medulla-core/ontology/infrastructure-v1.ttl"),
    include_str!("../../medulla-core/ontology/company-v1.ttl"),
];
const SHAPES: &str = include_str!("../../medulla-core/ontology/archimate-v1.shapes.ttl");
const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> . \
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \n";

fn objects(subject: &str, predicate: &str) -> Vec<String> {
    let (subject, predicate) = (format!("{KG}{subject}"), predicate.to_owned());
    ONTOLOGY
        .iter()
        .flat_map(|doc| parse_turtle(doc).unwrap())
        .filter(|t| t.subject.to_string() == format!("<{subject}>"))
        .filter(|t| t.predicate.to_string() == format!("<{predicate}>"))
        .map(|t| match t.object {
            medulla_rdf::oxrdf::Term::Literal(l) => l.value().to_owned(),
            other => other
                .to_string()
                .trim_matches(|c| c == '<' || c == '>')
                .to_owned(),
        })
        .collect()
}

/// The nearest annotated type of `class`, walking `rdfs:subClassOf`.
fn resolve(class: &str) -> Vec<String> {
    let mut current = class.to_owned();
    for _ in 0..16 {
        let found = objects(&current, &format!("{KG}archimateType"));
        if !found.is_empty() {
            return found;
        }
        let parents = objects(&current, "http://www.w3.org/2000/01/rdf-schema#subClassOf");
        match parents.iter().find_map(|p| p.strip_prefix(KG)) {
            Some(parent) => current = parent.to_owned(),
            None => return Vec::new(),
        }
    }
    Vec::new()
}

// spec: MEDULLA-CQ-ORCH-R705.1
#[test]
fn mapped_classes_resolve_one_type() {
    for (class, expected) in [
        ("BusinessProcess", "BusinessProcess"),
        ("ApplicationComponent", "ApplicationComponent"),
        ("BusinessCapability", "Capability"),
        ("Service", "ApplicationService"),
    ] {
        assert_eq!(resolve(class), vec![expected.to_owned()], "{class}");
    }
    // A subclass inherits the nearest annotated type.
    assert_eq!(resolve("BusinessTask"), vec!["BusinessProcess".to_owned()]);
    assert_eq!(
        objects("realizesProcess", &format!("{KG}archimateRelationshipType")),
        vec!["Realization".to_owned()]
    );
    assert_eq!(
        objects("usesTool", &format!("{KG}archimateRelationshipType")),
        vec!["Serving".to_owned()]
    );

    // The annotations conform to the shapes; two on one class are refused.
    let data = ONTOLOGY[0];
    assert!(validate_turtle(SHAPES, data).unwrap().conforms);
    let doubled = format!(
        "{PREFIXES}:Thing :archimateType \"BusinessActor\" . :Thing :archimateType \"Node\" ."
    );
    assert!(!validate_turtle(SHAPES, &doubled).unwrap().conforms);
    let single = format!("{PREFIXES}:Thing :archimateType \"BusinessActor\" .");
    assert!(validate_turtle(SHAPES, &single).unwrap().conforms);
}
