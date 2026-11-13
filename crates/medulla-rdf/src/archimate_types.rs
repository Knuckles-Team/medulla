//! The ArchiMate type map read from the ontology (MEDULLA-CQ-ORCH-R705.6).
//!
//! The `:archimateType` and `:archimateRelationshipType` annotations of the
//! architecture ontologies, with the `rdfs:subClassOf` parents that let a
//! subclass inherit the nearest annotation, resolved to the plain maps the
//! exchange mapping and writer read. The Turtle is the one source of truth:
//! there is no second table.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use medulla_core::architecture_exchange::ArchimateTypeMap;
use oxrdf::Term;

use crate::mapping::parse_turtle;

const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// The ontology documents that carry the architecture annotations.
const ONTOLOGY: &[&str] = &[
    include_str!("../../medulla-core/ontology/archimate-v1.ttl"),
    include_str!("../../medulla-core/ontology/orchestration-v1.ttl"),
    include_str!("../../medulla-core/ontology/infrastructure-v1.ttl"),
    include_str!("../../medulla-core/ontology/company-v1.ttl"),
];

fn core_local(iri: &str) -> Option<String> {
    iri.trim_matches(|c| c == '<' || c == '>')
        .strip_prefix(CORE)
        .map(str::to_owned)
}

/// Resolve a type map from Turtle documents. A class or edge annotated twice
/// keeps the first label in name order, so the result does not depend on
/// document order.
pub fn type_map_from_turtle(documents: &[&str]) -> Result<ArchimateTypeMap, String> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let type_predicate = format!("<{CORE}archimateType>");
    let edge_predicate = format!("<{CORE}archimateRelationshipType>");
    let sub_class = format!("<{SUB_CLASS_OF}>");
    for document in documents {
        for triple in parse_turtle(document)? {
            let predicate = triple.predicate.to_string();
            let Some(subject) = core_local(&triple.subject.to_string()) else {
                continue;
            };
            if predicate == type_predicate || predicate == edge_predicate {
                let Term::Literal(literal) = &triple.object else {
                    continue;
                };
                let target = if predicate == type_predicate {
                    &mut classes
                } else {
                    &mut edges
                };
                target
                    .entry(literal.value().to_owned())
                    .or_default()
                    .push(subject);
            } else if predicate == sub_class {
                if let Some(parent) = core_local(&triple.object.to_string()) {
                    parents.entry(subject).or_insert(parent);
                }
            }
        }
    }
    let first = |map: BTreeMap<String, Vec<String>>| {
        map.into_iter()
            .filter_map(|(key, mut labels)| {
                labels.sort();
                labels.into_iter().next().map(|label| (key, label))
            })
            .collect()
    };
    Ok(ArchimateTypeMap {
        classes: first(classes),
        edges: first(edges),
        parents,
    })
}

/// The type map of the shipped architecture ontologies, read once.
pub fn archimate_type_map() -> &'static ArchimateTypeMap {
    static MAP: OnceLock<ArchimateTypeMap> = OnceLock::new();
    MAP.get_or_init(|| {
        type_map_from_turtle(ONTOLOGY).expect("the shipped architecture ontologies parse")
    })
}
