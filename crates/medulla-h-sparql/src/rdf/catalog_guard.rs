//! The catalog write guard on the RDF write route (MEDULLA-TYPED-PACKS-R108.4).
//!
//! `AddTriples` runs [`guard_catalog_write`] before the write lands. The class
//! hierarchy and the marking of each person field come from the embedded catalog
//! ontology, so there is one source for both: the subclass closure of `cat:Asset`
//! and the `catn:fieldMarking` annotation of each property. The markings the
//! writer holds come from the verified request scopes: a scope `marking:<name>`
//! grants the marking `<name>`. A wildcard scope grants none, so a marking is
//! always an explicit grant.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use medulla_catalog::emit::{Object, Triple, CAT};
use medulla_catalog::guard::{
    check_catalog_write, flag_ownerless, CatalogGuardContext, CatalogGuardError,
};
use medulla_identity_types::tenancy::Marking;
use medulla_rdf::oxrdf::{self, NamedOrBlankNode, Term};
use medulla_server_api::carrier::GraphReadAuthority;

const CATALOG_ONTOLOGY: &str =
    include_str!("../../../medulla-core/ontology/metadata_catalog-v1.ttl");
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
/// The scope prefix that grants one marking to a writer.
pub(super) const MARKING_SCOPE_PREFIX: &str = medulla_server_api::read_scope::MARKING_SCOPE_PREFIX;

/// What the embedded catalog ontology says about classes and field markings.
struct CatalogRules {
    asset_classes: BTreeSet<String>,
    field_markings: BTreeMap<String, Marking>,
}

fn rules() -> &'static CatalogRules {
    static RULES: OnceLock<CatalogRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let triples = medulla_rdf::mapping::parse_turtle(CATALOG_ONTOLOGY)
            .expect("the embedded catalog ontology parses");
        rules_from(&triples)
    })
}

fn rules_from(triples: &[oxrdf::Triple]) -> CatalogRules {
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut field_markings = BTreeMap::new();
    let marking_predicate = format!("{CAT}fieldMarking");
    for triple in triples {
        let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            continue;
        };
        match (triple.predicate.as_str(), &triple.object) {
            (RDFS_SUBCLASS_OF, Term::NamedNode(parent)) => children
                .entry(parent.as_str().to_string())
                .or_default()
                .push(subject.as_str().to_string()),
            (predicate, Term::Literal(value)) if predicate == marking_predicate => {
                if let Ok(marking) = Marking::new(value.value()) {
                    field_markings.insert(subject.as_str().to_string(), marking);
                }
            }
            _ => {}
        }
    }
    let root = format!("{CAT}Asset");
    let mut asset_classes: BTreeSet<String> = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(class) = pending.pop() {
        if asset_classes.insert(class.clone()) {
            pending.extend(children.remove(&class).unwrap_or_default());
        }
    }
    CatalogRules {
        asset_classes,
        field_markings,
    }
}

/// The markings the verified request holds, read from its scopes.
fn held_markings(authority: Option<&GraphReadAuthority>) -> BTreeSet<Marking> {
    authority
        .and_then(GraphReadAuthority::carrier)
        .map(|carrier| {
            carrier
                .scopes()
                .iter()
                .filter_map(|scope| scope.strip_prefix(MARKING_SCOPE_PREFIX))
                .filter_map(|name| Marking::new(name).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn catalog_triples(triples: &[oxrdf::Triple]) -> Vec<Triple> {
    triples
        .iter()
        .filter_map(|triple| {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                return None;
            };
            let object = match &triple.object {
                Term::NamedNode(node) => Object::Iri(node.as_str().to_string()),
                Term::Literal(literal) => Object::Literal(literal.value().to_string()),
                _ => return None,
            };
            Some(Triple {
                subject: subject.as_str().to_string(),
                predicate: triple.predicate.as_str().to_string(),
                object,
            })
        })
        .collect()
}

fn to_rdf(triple: &Triple) -> Option<oxrdf::Triple> {
    let subject = oxrdf::NamedNode::new(triple.subject.clone()).ok()?;
    let predicate = oxrdf::NamedNode::new(triple.predicate.clone()).ok()?;
    let object: Term = match &triple.object {
        Object::Iri(iri) => oxrdf::NamedNode::new(iri.clone()).ok()?.into(),
        Object::Literal(value) => oxrdf::Literal::new_simple_literal(value.clone()).into(),
    };
    Some(oxrdf::Triple::new(subject, predicate, object))
}

/// Check one `AddTriples` batch and add the `owner_missing` flag to every asset
/// node it declares without an owner. A refusal leaves the batch unwritten.
pub(super) fn guard_catalog_write(
    triples: &mut Vec<oxrdf::Triple>,
    authority: Option<&GraphReadAuthority>,
) -> Result<(), CatalogGuardError> {
    let rules = rules();
    let held = held_markings(authority);
    let catalog = catalog_triples(triples);
    let is_asset_class = |class: &str| rules.asset_classes.contains(class);
    let field_marking = |predicate: &str| rules.field_markings.get(predicate).cloned();
    check_catalog_write(
        &catalog,
        &CatalogGuardContext {
            is_asset_class: &is_asset_class,
            held: &held,
            field_marking: &field_marking,
        },
    )?;
    triples.extend(flag_ownerless(&catalog).iter().filter_map(to_rdf));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(turtle: &str) -> Vec<oxrdf::Triple> {
        medulla_rdf::mapping::parse_turtle(turtle).expect("turtle parses")
    }

    // spec: MEDULLA-TYPED-PACKS-R108.4
    #[test]
    fn the_embedded_ontology_gives_the_asset_closure_and_the_person_field_markings() {
        let rules = rules();
        assert!(rules.asset_classes.contains(&format!("{CAT}Asset")));
        assert!(rules.asset_classes.contains(&format!("{CAT}PersonAsset")));
        assert!(rules.asset_classes.contains(&format!("{CAT}ToolAsset")));
        assert!(!rules
            .asset_classes
            .contains(&format!("{CAT}StewardshipRole")));
        assert_eq!(
            rules
                .field_markings
                .get(&format!("{CAT}email"))
                .map(Marking::as_str),
            Some("pii")
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R108.4
    #[test]
    fn a_batch_is_refused_or_flagged_without_a_held_marking() {
        let head = format!("@prefix cat: <{CAT}> .\n");
        let orphan = parse(&format!("{head}<urn:medulla:asset:x:1> a cat:Widget .\n"));
        let mut orphan_batch = orphan;
        let error = guard_catalog_write(&mut orphan_batch, None).unwrap_err();
        assert_eq!(error.code(), "ASSET_CLASS_ORPHAN");

        let mut marked = parse(&format!(
            "{head}<urn:medulla:asset:person:a> a cat:PersonAsset ; cat:email \"a@example.test\" .\n"
        ));
        let error = guard_catalog_write(&mut marked, None).unwrap_err();
        assert_eq!(error.code(), "FIELD_MARKED");

        let mut ownerless = parse(&format!(
            "{head}<urn:medulla:asset:product:1> a cat:Product .\n"
        ));
        let before = ownerless.len();
        guard_catalog_write(&mut ownerless, None)
            .expect("an ownerless node is flagged, not refused");
        assert_eq!(ownerless.len(), before + 1);
    }
}
