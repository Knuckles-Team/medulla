//! The process and decision vocabulary's core roll-up (MEDULLA-ORCH-VOCAB-R014).
#![cfg(feature = "rdf")]

mod common;

use std::collections::BTreeSet;

use common::{parse_all, KG, RDFS};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";

const FOUNDATION: &str = include_str!("../../medulla-core/ontology/core-foundation-v1.ttl");
const DMN: &str = include_str!("../../medulla-core/ontology/dmn-v1.ttl");

fn named(triple: &Triple) -> Option<&str> {
    match &triple.subject {
        NamedOrBlankNode::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

fn parents<'a>(triples: &'a [Triple], class: &str) -> BTreeSet<&'a str> {
    triples
        .iter()
        .filter(|t| named(t) == Some(class) && t.predicate.as_str() == format!("{RDFS}subClassOf"))
        .filter_map(|t| match &t.object {
            Term::NamedNode(node) => Some(node.as_str()),
            _ => None,
        })
        .collect()
}

// spec: MEDULLA-ORCH-VOCAB-R014.3
#[test]
fn every_dmn_class_has_one_core_parent() {
    let triples = parse_all(&[FOUNDATION, DMN]);
    let foundation = parse_all(&[FOUNDATION]);
    let declared_in_foundation = |class: &str| {
        foundation.iter().any(|t| {
            named(t) == Some(class)
                && t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
        })
    };
    let dmn_classes: BTreeSet<String> = parse_all(&[DMN])
        .iter()
        .filter(|t| {
            t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
        })
        .filter_map(|t| named(t).map(str::to_owned))
        .collect();
    assert_eq!(dmn_classes.len(), 7, "{dmn_classes:?}");
    for class in &dmn_classes {
        assert!(class.starts_with(&format!("{KG}Dmn")), "{class}");
        let parents = parents(&triples, class);
        assert_eq!(parents.len(), 1, "{class} has parents {parents:?}");
        let parent = *parents.iter().next().unwrap();
        // The parent is a core class, or the one dmn class that is itself rooted in one.
        assert!(
            declared_in_foundation(parent) || dmn_classes.contains(parent),
            "{class} extends {parent}, which is not a core class"
        );
    }
    for (class, parent) in [
        ("DmnDecisionRequirements", "DecisionLogic"),
        ("DmnDecision", "DecisionLogic"),
        ("DmnDecisionTable", "DecisionLogic"),
        ("DmnRule", "DecisionRule"),
        ("DmnInputClause", "Concept"),
        ("DmnOutputClause", "Concept"),
        ("DmnInputData", "Concept"),
    ] {
        assert_eq!(
            parents(&triples, &format!("{KG}{class}")),
            BTreeSet::from([format!("{KG}{parent}").as_str()])
        );
    }
}
