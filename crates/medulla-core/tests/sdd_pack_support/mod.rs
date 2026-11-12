//! Shared checks for the spec-driven development ontology tests: parse a
//! registered core source, collect declared classes and parents, and the
//! `PACK_CLASS_UNROOTED` shape rule.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};

pub const KG: &str = "https://knuckles-team.github.io/ontology/core#";
pub const BFO: &str = "http://purl.obolibrary.org/obo/BFO_";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const FOUNDATION: &str = "core:foundation@1";

pub struct Hierarchy {
    pub parents: BTreeMap<String, BTreeSet<String>>,
    pub core: BTreeSet<String>,
}

/// Classes declared by one document and the parents of every class in it.
pub fn declared_and_parents(
    document: &str,
) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
    let mut declared = BTreeSet::new();
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for triple in parse_turtle(document).expect("document parses") {
        let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            continue;
        };
        match (triple.predicate.as_str(), &triple.object) {
            (RDF_TYPE, Term::NamedNode(object)) if object.as_str() == OWL_CLASS => {
                declared.insert(subject.as_str().to_string());
            }
            (SUBCLASS_OF, Term::NamedNode(object)) => {
                parents
                    .entry(subject.as_str().to_string())
                    .or_default()
                    .insert(object.as_str().to_string());
            }
            _ => {}
        }
    }
    (declared, parents)
}

/// The registered ontology document of one core source.
pub fn registered(sources: &GraphSchemaSources, id: &str) -> String {
    sources
        .ontologies()
        .find(|(source, _)| *source == id)
        .unwrap_or_else(|| panic!("core source {id} is not registered"))
        .1
        .to_string()
}

/// Parent links across every registered module, and the core class set.
pub fn hierarchy(sources: &GraphSchemaSources) -> Hierarchy {
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut core = BTreeSet::new();
    for (id, document) in sources.ontologies() {
        let (declared, found) = declared_and_parents(document);
        if id == FOUNDATION {
            core = declared;
        }
        for (class, set) in found {
            parents.entry(class).or_default().extend(set);
        }
    }
    assert!(!core.is_empty(), "the foundation declares the core classes");
    Hierarchy { parents, core }
}

impl Hierarchy {
    pub fn reaches_core(&self, class: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut pending = vec![class.to_string()];
        while let Some(next) = pending.pop() {
            for parent in self.parents.get(&next).into_iter().flatten() {
                if self.core.contains(parent) {
                    return true;
                }
                if seen.insert(parent.clone()) {
                    pending.push(parent.clone());
                }
            }
        }
        false
    }

    /// The shape rule: every class `document` declares has an
    /// `rdfs:subClassOf` path to a core class, or the document is refused
    /// with `PACK_CLASS_UNROOTED` naming the first class without one.
    pub fn refuse_unrooted(&self, document: &str) -> Result<usize, String> {
        let (declared, own_parents) = declared_and_parents(document);
        let mut local = Hierarchy {
            parents: self.parents.clone(),
            core: self.core.clone(),
        };
        for (class, set) in own_parents {
            local.parents.entry(class).or_default().extend(set);
        }
        for class in &declared {
            if !local.core.contains(class) && !local.reaches_core(class) {
                return Err(format!(
                    "PACK_CLASS_UNROOTED: class {class} has no rdfs:subClassOf path to a core class"
                ));
            }
        }
        Ok(declared.len())
    }
}
