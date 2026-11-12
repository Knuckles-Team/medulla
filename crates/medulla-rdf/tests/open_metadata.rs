//! Open metadata type names resolve to exactly one catalog class
//! (MEDULLA-TYPED-PACKS-R110.1).
#![cfg(feature = "owl")]

mod common;

use common::parse_all;
use medulla_rdf::oxrdf::{NamedNode, Term, Triple};
use std::collections::BTreeMap;

const OPEN_METADATA_TYPE: &str =
    "https://knuckles-team.github.io/ontology/catalog#openMetadataType";
const CAT: &str = "http://knuckles.team/kg/catalog#";

fn documents() -> Vec<Triple> {
    parse_all(&[include_str!(
        "../../medulla-core/ontology/metadata_catalog-v1.ttl"
    )])
}

/// Every open metadata type name with the classes that carry it.
fn mapping(triples: &[Triple]) -> BTreeMap<String, Vec<String>> {
    let predicate = NamedNode::new(OPEN_METADATA_TYPE).unwrap();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for triple in triples.iter().filter(|t| t.predicate == predicate) {
        if let Term::Literal(literal) = &triple.object {
            out.entry(literal.value().to_string()).or_default().push(
                triple
                    .subject
                    .to_string()
                    .trim_matches(['<', '>'])
                    .to_string(),
            );
        }
    }
    out
}

// spec: MEDULLA-TYPED-PACKS-R110.1
#[test]
fn mapped_types_resolve_one_class() {
    let map = mapping(&documents());
    let expected = [
        ("GlossaryTerm", "GlossaryTerm"),
        ("Glossary", "Catalog"),
        ("DataSet", "DataResource"),
        ("DataFlow", "LineageEdge"),
        ("Confidentiality", "ConfidentialityClassification"),
        ("Criticality", "CriticalityClassification"),
    ];
    for (type_name, class) in expected {
        assert_eq!(
            map.get(type_name),
            Some(&vec![format!("{CAT}{class}")]),
            "open metadata type {type_name} must resolve to exactly one class"
        );
    }
    assert_eq!(map.len(), expected.len(), "no unexpected mapped type");
    // A type name without an annotation is unmapped.
    assert!(!map.contains_key("Asset"));
}
