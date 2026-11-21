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
    check_batch_size, check_catalog_write, flag_ownerless, CatalogGuardContext, CatalogGuardError,
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
    let max = medulla_server_api::knob_config::knobs().catalog_onboard_batch_max_triples as usize;
    check_batch_size(&catalog, max)?;
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
    // The schema digest is the engine's to write (MEDULLA-TYPED-PACKS-R805.3): a writer's own
    // value is dropped and the digest of the batch's fields stands in its place.
    let digest_predicate = format!("{CAT}schemaDigest");
    triples.retain(|triple| triple.predicate.as_str() != digest_predicate);
    triples.extend(schema_digest_triples(&catalog).iter().filter_map(to_rdf));
    Ok(())
}

/// Refuse an article write that lowers the stored `cat:articleVersion`, or that creates an
/// article in any status but `draft` (MEDULLA-TYPED-PACKS-R108.13). The refusal leads with
/// `ARTICLE_VERSION_INVALID`; a refused batch commits nothing.
pub(super) fn guard_article_versions(
    triples: &[oxrdf::Triple],
    core: &medulla_graph_core::graph::GraphCore,
) -> Result<(), String> {
    let article = format!("{CAT}Article");
    let version_key = format!("{CAT}articleVersion");
    let status_key = format!("{CAT}articleStatus");
    let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let literal_of = |subject: &NamedOrBlankNode, predicate: &str| {
        triples.iter().find_map(|t| match (&t.object, t.predicate.as_str()) {
            (Term::Literal(value), p) if p == predicate && &t.subject == subject => {
                Some(value.value().to_string())
            }
            _ => None,
        })
    };
    let refuse = |detail: String| format!("ARTICLE_VERSION_INVALID: {detail}");
    for triple in triples {
        let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            continue;
        };
        if triple.predicate.as_str() != rdf_type
            || !matches!(&triple.object, Term::NamedNode(class) if class.as_str() == article)
        {
            continue;
        }
        let incoming = literal_of(&triple.subject, &version_key).and_then(|v| v.parse::<u64>().ok());
        let stored = core
            .get_node_properties(&format!("<{}>", subject.as_str()))
            .and_then(|bytes| medulla_types::msgpack::decode_property_value(&bytes).ok());
        match stored {
            Some(node) => {
                let held = node
                    .get(&version_key)
                    .or_else(|| node.get("articleVersion"))
                    .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
                if let (Some(incoming), Some(held)) = (incoming, held) {
                    if incoming < held {
                        return Err(refuse(format!(
                            "article {} is at version {held}; a write at version {incoming} is refused",
                            subject.as_str()
                        )));
                    }
                }
            }
            None => {
                let status = literal_of(&triple.subject, &status_key);
                if status.as_deref() != Some("draft") {
                    return Err(refuse(format!(
                        "a new article {} starts in draft",
                        subject.as_str()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The stored property object of the node an IRI names, and the key it is stored under.
fn stored_node(
    core: &medulla_graph_core::graph::GraphCore,
    iri: &str,
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    [format!("<{iri}>"), iri.to_string()].into_iter().find_map(|key| {
        let bytes = core.get_node_properties(&key)?;
        match medulla_types::msgpack::decode_property_value(&bytes).ok()? {
            serde_json::Value::Object(map) => Some((key, map)),
            _ => None,
        }
    })
}

/// The bare IRI of a stored node key.
fn bare_iri(key: &str) -> &str {
    key.strip_prefix('<').and_then(|k| k.strip_suffix('>')).unwrap_or(key)
}

/// Whether the stored edge `from -> to` carries the relationship local name `name`.
fn has_relationship(
    core: &medulla_graph_core::graph::GraphCore,
    from: &str,
    to: &str,
    name: &str,
) -> bool {
    core.get_edge_properties(from, to).iter().any(|blob| {
        medulla_types::msgpack::decode_property_value(blob)
            .ok()
            .and_then(|v| v.get("relationship").and_then(|r| r.as_str().map(str::to_string)))
            .is_some_and(|rel| rel.trim_matches(['<', '>']).rsplit(['#', '/']).next() == Some(name))
    })
}

/// Materialise `cat:describedBest` on every article the batch touches (MEDULLA-TYPED-PACKS-R108.16).
/// An article is touched when the batch writes it with `cat:aboutAsset` edges, or writes an asset
/// that a stored article describes. The value is the ranking text of the described assets, so the
/// SQL `catalog_relevance` function and the SPARQL `cat:relevance` function read one stored value
/// and score an article by the best of it for the query text. The property is the engine's to
/// write: a writer's own value is dropped.
pub(super) fn materialize_described_best(
    triples: &mut Vec<oxrdf::Triple>,
    core: &medulla_graph_core::graph::GraphCore,
) {
    use medulla_catalog_types::catalog_relevance as cr;
    use serde_json::{Map, Value};
    let about = format!("{CAT}aboutAsset");
    let predicate = format!("{CAT}describedBest");
    triples.retain(|triple| triple.predicate.as_str() != predicate);
    let mut batch_text: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut batch_about: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut written: BTreeSet<String> = BTreeSet::new();
    for triple in triples.iter() {
        let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
            continue;
        };
        written.insert(subject.as_str().to_string());
        match &triple.object {
            Term::Literal(value) => {
                let cell = batch_text
                    .entry(subject.as_str().to_string())
                    .or_default()
                    .entry(triple.predicate.as_str().to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(values) = cell {
                    values.push(Value::String(value.value().to_string()));
                }
            }
            Term::NamedNode(object) if triple.predicate.as_str() == about => {
                batch_about
                    .entry(subject.as_str().to_string())
                    .or_default()
                    .insert(object.as_str().to_string());
            }
            _ => {}
        }
    }
    // Articles to refresh: those the batch writes with edges, and the stored articles that
    // describe an asset the batch writes.
    let mut articles: BTreeSet<String> = batch_about.keys().cloned().collect();
    for iri in &written {
        let Some((key, _)) = stored_node(core, iri) else {
            continue;
        };
        for from in core.get_predecessors(&key).unwrap_or_default() {
            if has_relationship(core, &from, &key, "aboutAsset") {
                articles.insert(bare_iri(&from).to_string());
            }
        }
    }
    let asset_entry = |iri: &str| -> Option<Map<String, Value>> {
        let stored = stored_node(core, iri);
        let in_batch = batch_text.get(iri);
        if stored.is_none() && in_batch.is_none() {
            return None;
        }
        let mut merged = stored.as_ref().map(|(_, map)| map.clone()).unwrap_or_default();
        for (key, value) in in_batch.into_iter().flatten() {
            merged.insert(key.clone(), value.clone());
        }
        let mut term_labels = Vec::new();
        if let Some((key, _)) = &stored {
            for from in core.get_predecessors(key).unwrap_or_default() {
                if !has_relationship(core, &from, key, "definesAsset") {
                    continue;
                }
                if let Some((_, term)) = stored_node(core, bare_iri(&from)) {
                    term_labels.extend(cr::term_labels(&term));
                }
            }
        }
        Some(cr::described_entry(&merged, &term_labels))
    };
    for article in articles {
        let mut targets = batch_about.get(&article).cloned().unwrap_or_default();
        if let Some((key, _)) = stored_node(core, &article) {
            for to in core.get_successors(&key).unwrap_or_default() {
                if has_relationship(core, &key, &to, "aboutAsset") {
                    targets.insert(bare_iri(&to).to_string());
                }
            }
        }
        let entries: Vec<Map<String, Value>> =
            targets.iter().filter_map(|target| asset_entry(target)).collect();
        let (Ok(subject), Ok(predicate_node)) = (
            oxrdf::NamedNode::new(article.clone()),
            oxrdf::NamedNode::new(predicate.clone()),
        ) else {
            continue;
        };
        triples.push(oxrdf::Triple::new(
            subject,
            predicate_node,
            oxrdf::Literal::new_simple_literal(cr::described_snapshot(&entries)),
        ));
    }
}

/// `cat:schemaDigest` for every `cat:DataResource` of the batch that names at least one field
/// with a `cat:columnName`. A field's type defaults to the empty string and its nullability to
/// `true` when the batch does not state them.
fn schema_digest_triples(catalog: &[Triple]) -> Vec<Triple> {
    use medulla_catalog_types::data_product::{schema_digest, DigestColumn};
    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let resource_class = Object::Iri(format!("{CAT}DataResource"));
    let has_field = format!("{CAT}hasField");
    fn literal<'a>(catalog: &'a [Triple], subject: &str, predicate: &str) -> Option<&'a str> {
        catalog
            .iter()
            .find(|t| t.subject == subject && t.predicate == predicate)
            .and_then(|t| match &t.object {
                Object::Literal(value) => Some(value.as_str()),
                Object::Iri(_) => None,
            })
    }
    let (name, kind, nullable) = (
        format!("{CAT}columnName"),
        format!("{CAT}columnType"),
        format!("{CAT}columnNullable"),
    );
    let resources: BTreeSet<&str> = catalog
        .iter()
        .filter(|t| t.predicate == RDF_TYPE && t.object == resource_class)
        .map(|t| t.subject.as_str())
        .collect();
    let mut out = Vec::new();
    for resource in resources {
        let columns: Vec<DigestColumn> = catalog
            .iter()
            .filter(|t| t.subject == resource && t.predicate == has_field)
            .filter_map(|t| match &t.object {
                Object::Iri(field) => Some(field.as_str()),
                Object::Literal(_) => None,
            })
            .filter_map(|field| {
                Some(DigestColumn {
                    name: literal(catalog, field, &name)?.to_string(),
                    type_name: literal(catalog, field, &kind).unwrap_or_default().to_string(),
                    nullable: literal(catalog, field, &nullable) != Some("false"),
                })
            })
            .collect();
        if !columns.is_empty() {
            out.push(Triple {
                subject: resource.to_string(),
                predicate: format!("{CAT}schemaDigest"),
                object: Object::Literal(schema_digest(&columns)),
            });
        }
    }
    out
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

    // spec: MEDULLA-TYPED-PACKS-R805.3
    #[test]
    fn ingest_writes_the_schema_digest_over_sorted_columns_and_drops_a_forged_one() {
        let head = format!("@prefix cat: <{CAT}> .\n");
        let batch = |order: &[(&str, &str)], forged: &str| {
            let mut text = format!(
                "{head}<urn:medulla:asset:resource:r> a cat:DataResource ; cat:hasOwner <urn:o:a> {forged} ;\n  cat:hasField <urn:medulla:asset:field:a>, <urn:medulla:asset:field:b> .\n"
            );
            for (id, ty) in order {
                text.push_str(&format!(
                    "<urn:medulla:asset:field:{id}> a cat:Field ; cat:hasOwner <urn:o:a> ; cat:columnName \"{id}\" ; cat:columnType \"{ty}\" ; cat:columnNullable \"false\" .\n"
                ));
            }
            let mut triples = parse(&text);
            guard_catalog_write(&mut triples, None).expect("the batch is accepted");
            triples
        };
        let digest_of = |triples: &[oxrdf::Triple]| -> Vec<String> {
            triples
                .iter()
                .filter(|t| t.predicate.as_str() == format!("{CAT}schemaDigest"))
                .map(|t| t.object.to_string())
                .collect()
        };
        let one = digest_of(&batch(&[("a", "int"), ("b", "text")], ""));
        let two = digest_of(&batch(&[("b", "text"), ("a", "int")], ""));
        assert_eq!(one.len(), 1);
        assert_eq!(one, two);
        assert!(one[0].starts_with("\"sha256:"));
        let forged = digest_of(&batch(&[("a", "int"), ("b", "text")], "; cat:schemaDigest \"sha256:forged\""));
        assert_eq!(forged, one);
        let changed = digest_of(&batch(&[("a", "bigint"), ("b", "text")], ""));
        assert_ne!(changed, one);
    }
}
