//! MEDULLA-TYPED-PACKS-R102.3: a pack ontology extends core classes and never
//! redeclares them.
//!
//! A pack reuses a core class by IRI and adds its own classes beneath it. An
//! axiom whose subject is a core class would change what the core class means
//! for every pack, so import refuses it as `ONTOLOGY_CORE_CONFLICT`. The check
//! sits beside the orphan check, which refuses a class with no core path.

use std::collections::BTreeSet;

use medulla_catalog_types::connector_pack::PackViolationCode;
use medulla_rdf::oxrdf::NamedOrBlankNode;

use super::{class_list_detail, parent_classes, IndexValidator};

/// The axiom predicates that alter the meaning of their subject class.
const CORE_AXIOMS: [&str; 4] = [
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2002/07/owl#complementOf",
    "http://www.w3.org/2002/07/owl#disjointWith",
    "http://www.w3.org/2002/07/owl#equivalentClass",
];

/// The core classes `documents` state an axiom about, sorted.
fn redeclared_core_classes(documents: &[&str], core: &BTreeSet<String>) -> Vec<String> {
    let mut found = BTreeSet::new();
    for text in documents {
        for triple in medulla_rdf::mapping::parse_turtle(text).unwrap_or_default() {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            if core.contains(subject.as_str()) && CORE_AXIOMS.contains(&triple.predicate.as_str()) {
                found.insert(subject.as_str().to_string());
            }
        }
    }
    found.into_iter().collect()
}

/// Refuse the pack when an ontology redeclares a core class with axioms.
pub(super) fn flag_core_conflicts(validator: &mut IndexValidator<'_>) {
    let documents: Vec<&str> = validator.ontologies.iter().map(String::as_str).collect();
    let conflicts = redeclared_core_classes(&documents, parent_classes());
    if conflicts.is_empty() {
        return;
    }
    let lead = format!(
        "{} core class(es) are redeclared with axioms; reuse them by IRI: ",
        conflicts.len()
    );
    let detail = class_list_detail(&lead, &conflicts);
    validator.reject(PackViolationCode::OntologyCoreConflict, None, &detail);
}
