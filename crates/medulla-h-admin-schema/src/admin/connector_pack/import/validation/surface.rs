//! MEDULLA-TYPED-PACKS-R102.5: the surface links a pack ontology declares must
//! resolve inside the pack.
//!
//! A surface link names a member of the pack, a connector, or a domain class of
//! the composed schema. Every refusal is raised before the import commits, so
//! a refused pack changes neither the head nor the graph.

use std::collections::BTreeSet;

use medulla_catalog_types::connector_pack::PackViolationCode;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};

use super::super::contract_rdf::segment;
use super::{parent_classes, IndexValidator};
use crate::admin::connector_pack::surface_vocab::{
    is_member_uri, link_object, member_connector, Object, CONNECTOR_BASE,
};

/// The most refusals one pack reports.
const MAX_REFUSALS: usize = 16;

type Refusal = (PackViolationCode, String);

/// What a surface link is checked against.
struct Scope<'a> {
    connector: &'a str,
    members: &'a BTreeSet<String>,
    classes: BTreeSet<String>,
}

impl Scope<'_> {
    /// A claim about `iri`: refused when it names another connector's member
    /// or connector node, or a member this pack lacks.
    fn claim(&self, iri: &str) -> Option<Refusal> {
        if let Some(owner) = iri.strip_prefix(CONNECTOR_BASE) {
            return (owner != segment(self.connector)).then(|| mismatch(iri));
        }
        if member_connector(iri).is_some_and(|owner| owner != self.connector) {
            return Some(mismatch(iri));
        }
        (is_member_uri(iri) && !self.members.contains(iri)).then(|| unresolved(iri))
    }

    /// The object of a link, by what its predicate requires.
    fn object(&self, required: Object, term: &Term) -> Option<Refusal> {
        let iri = match term {
            Term::NamedNode(node) => node.as_str(),
            _ => return Some(not_a_class("a blank or literal object")),
        };
        match required {
            Object::DomainClass => (!self.classes.contains(iri)).then(|| not_a_class(iri)),
            Object::Member => self
                .claim(iri)
                .or_else(|| (!self.members.contains(iri)).then(|| unresolved(iri))),
            Object::Open => None,
        }
    }

    fn check(&self, triple: &Triple) -> Option<Refusal> {
        let required = link_object(triple.predicate.as_str())?;
        let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            return None;
        };
        self.claim(subject.as_str())
            .or_else(|| self.object(required, &triple.object))
    }
}

fn mismatch(iri: &str) -> Refusal {
    (
        PackViolationCode::ImporterMismatch,
        format!("surface claim about another connector: {iri}"),
    )
}

fn unresolved(iri: &str) -> Refusal {
    (
        PackViolationCode::UnresolvedReference,
        format!("surface link names a component absent from the pack: {iri}"),
    )
}

fn not_a_class(iri: &str) -> Refusal {
    (
        PackViolationCode::ContractShaclViolation,
        format!("surface link names a domain class absent from the composed schema: {iri}"),
    )
}

/// The classes the composed schema holds: the core classes and every class a
/// pack ontology declares.
fn composed_classes(documents: &[&str]) -> BTreeSet<String> {
    let mut classes = parent_classes().clone();
    for text in documents {
        classes.extend(medulla_rdf::pack::declared_classes_and_ontologies(text).0);
    }
    classes
}

/// Refuse the pack for every unresolved surface link its ontologies declare.
pub(super) fn check_surface(validator: &mut IndexValidator<'_>) {
    let documents: Vec<&str> = validator.ontologies.iter().map(String::as_str).collect();
    if documents.is_empty() {
        return;
    }
    let scope = Scope {
        connector: validator.request.index.connector.as_str(),
        members: &validator.uris,
        classes: composed_classes(&documents),
    };
    let mut refusals = BTreeSet::new();
    for text in &documents {
        let triples = medulla_rdf::mapping::parse_turtle(text).unwrap_or_default();
        refusals.extend(triples.iter().filter_map(|triple| scope.check(triple)));
    }
    for (code, detail) in refusals.into_iter().take(MAX_REFUSALS) {
        validator.reject(code, None, &detail);
    }
}
