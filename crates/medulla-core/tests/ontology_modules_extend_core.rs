//! Shipped modules extend core classes and never import core
//! (MEDULLA-TYPED-PACKS-R071.2).
//!
//! The immutable core catalog is the corpus: no module declares `owl:imports`
//! of the core document, and every class a module declares reaches a class of
//! the core foundation through `rdfs:subClassOf`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_core::ontology_codegen::refuse_modules_importing_core;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const FOUNDATION: &str = "core:foundation@1";

struct Hierarchy {
    /// Named classes each source declares.
    declared: BTreeMap<String, BTreeSet<String>>,
    parents: BTreeMap<String, BTreeSet<String>>,
}

fn hierarchy(sources: &GraphSchemaSources) -> Hierarchy {
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, document) in sources.ontologies() {
        let triples = parse_turtle(document).expect("a core module parses");
        for triple in triples {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            match (triple.predicate.as_str(), &triple.object) {
                (RDF_TYPE, Term::NamedNode(object)) if object.as_str() == OWL_CLASS => {
                    declared
                        .entry(id.to_string())
                        .or_default()
                        .insert(subject.as_str().to_string());
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
    }
    Hierarchy { declared, parents }
}

impl Hierarchy {
    fn reaches(&self, class: &str, targets: &BTreeSet<String>) -> bool {
        let mut seen = BTreeSet::new();
        let mut pending = vec![class.to_string()];
        while let Some(next) = pending.pop() {
            for parent in self.parents.get(&next).into_iter().flatten() {
                if targets.contains(parent) {
                    return true;
                }
                if seen.insert(parent.clone()) {
                    pending.push(parent.clone());
                }
            }
        }
        false
    }
}

// spec: MEDULLA-TYPED-PACKS-R071.2
#[test]
fn no_module_imports_core_and_every_class_reaches_core() {
    let sources = GraphSchemaSources::default();
    refuse_modules_importing_core(sources.ontologies().filter(|(id, _)| *id != FOUNDATION))
        .expect("no shipped module imports the core document");

    let hierarchy = hierarchy(&sources);
    let core: BTreeSet<String> = hierarchy
        .declared
        .get(FOUNDATION)
        .expect("the foundation module declares the core classes")
        .clone();
    assert!(!core.is_empty());

    let mut rooted = 0usize;
    for (id, classes) in &hierarchy.declared {
        if id == FOUNDATION {
            continue;
        }
        for class in classes.iter().filter(|class| !core.contains(*class)) {
            assert!(
                hierarchy.reaches(class, &core),
                "{id}: class {class} has no rdfs:subClassOf path to a core class"
            );
            rooted += 1;
        }
    }
    assert!(rooted > 0);
}
