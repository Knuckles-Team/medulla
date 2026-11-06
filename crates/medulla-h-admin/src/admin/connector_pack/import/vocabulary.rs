//! The pinned vocabulary a pack's crosswalk and concept-path entries resolve
//! against (MEDULLA-TYPED-PACKS-R071.1.3, R071.100).
//!
//! The pinned revision is the composed schema digest of the engine-owned core
//! catalog. The class set is read from the composed schema (core plus the
//! pack's own ontology entries) by `validate_and_compose`; the selected source
//! set is the core catalog's classes; the hierarchy is the `rdfs:subClassOf`
//! relation of the composed schema. Nothing is fetched or adopted from outside.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::connector_pack::{PackEntry, PackEntryKind};

/// The composed vocabulary one import validates against.
#[derive(Debug, Default)]
pub(crate) struct PinnedVocabulary {
    /// Composed schema digest of the core catalog: the pinned revision.
    pub(crate) revision: String,
    /// Every composed class with its upper (BFO) category.
    pub(crate) categories: BTreeMap<String, String>,
    /// Classes of the core catalog: the authorized source set.
    pub(crate) core_classes: BTreeSet<String>,
    /// Named direct `rdfs:subClassOf` parents of each composed class.
    pub(crate) parents: BTreeMap<String, BTreeSet<String>>,
}

impl PinnedVocabulary {
    /// Every strict ancestor of `class` in the composed hierarchy.
    pub(crate) fn ancestors(&self, class: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut pending = vec![class.to_string()];
        while let Some(next) = pending.pop() {
            for parent in self.parents.get(&next).into_iter().flatten() {
                if found.insert(parent.clone()) {
                    pending.push(parent.clone());
                }
            }
        }
        found
    }
}

/// Whether the pack carries a kind that needs the pinned vocabulary.
pub(super) fn needs_vocabulary(all: &[&PackEntry]) -> bool {
    all.iter().any(|entry| {
        matches!(
            entry.kind,
            PackEntryKind::Crosswalk | PackEntryKind::ConceptPath
        )
    })
}

const BFO: &str = "http://purl.obolibrary.org/obo/BFO_";

/// The upper category of a class: the first BFO ancestor group that matches.
fn upper_category(class: &str, vocabulary: &BTreeMap<String, BTreeSet<String>>) -> &'static str {
    let mut lineage = BTreeSet::from([class.to_string()]);
    let mut pending = vec![class.to_string()];
    while let Some(next) = pending.pop() {
        for parent in vocabulary.get(&next).into_iter().flatten() {
            if lineage.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    let has = |ids: &[&str]| ids.iter().any(|id| lineage.contains(&format!("{BFO}{id}")));
    if has(&["0000015"]) {
        "process"
    } else if has(&["0000008", "0000038", "0000148"]) {
        "temporal_region"
    } else if has(&[
        "0000002", "0000004", "0000016", "0000017", "0000019", "0000020", "0000031", "0000040",
    ]) {
        "continuant"
    } else if has(&["0000003"]) {
        "occurrent"
    } else {
        "unclassified"
    }
}

#[cfg(feature = "shacl")]
mod compose {
    use super::*;
    use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
    use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
    const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

    fn hierarchy(triples: &[Triple]) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
        let mut classes = BTreeSet::new();
        let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for triple in triples {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            match (triple.predicate.as_str(), &triple.object) {
                (RDF_TYPE, Term::NamedNode(object)) if object.as_str() == OWL_CLASS => {
                    classes.insert(subject.as_str().to_string());
                }
                (RDFS_SUBCLASS_OF, Term::NamedNode(object)) => {
                    parents
                        .entry(subject.as_str().to_string())
                        .or_default()
                        .insert(object.as_str().to_string());
                }
                _ => {}
            }
        }
        (classes, parents)
    }

    /// Compose core plus the pack's own ontology entries and read the pinned
    /// vocabulary from the result.
    pub(in super::super) fn pin(
        request: &medulla_catalog_types::connector_pack::ConnectorPackImportRequest,
        archive: &[u8],
        all: &[&PackEntry],
    ) -> Result<PinnedVocabulary, String> {
        let core_sources = GraphSchemaSources::default();
        let mut documents = Vec::new();
        for entry in all.iter().filter(|e| e.kind == PackEntryKind::Ontology) {
            let bytes = super::super::validation::section_bytes(archive, &entry.body)?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| "MALFORMED_BODY: ontology is not UTF-8".to_string())?;
            documents.push(crate::graph_schema::PackSchemaDocument {
                uri: entry.uri.clone(),
                body: std::sync::Arc::from(text),
            });
        }
        let ontology = crate::graph_schema::compose_pack_documents("ONTOLOGY", documents)?;
        let mut sources = core_sources;
        if ontology.is_some() {
            let source = GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: request.index.connector.as_str().to_string(),
                    record_id: format!("import:{}", request.index.pack_digest.to_hex()),
                },
                None,
                ontology,
                0,
            )?;
            sources.attach_dynamic(format!("pack:{}", request.index.connector.as_str()), source)?;
        }
        pin_composed(&GraphSchemaSources::default(), &sources)
    }

    /// Read the pinned vocabulary from `sources`: the revision and the
    /// authorized source set come from the core catalog, the classes, upper
    /// categories and hierarchy from the composed schema. The import and the
    /// activation of a pack both pin through here.
    pub(crate) fn pin_composed(
        core_sources: &GraphSchemaSources,
        sources: &GraphSchemaSources,
    ) -> Result<PinnedVocabulary, String> {
        let revision = core_sources.composed_digest().to_hex();
        let core = medulla_server_api::schema_compose::validate_and_compose(core_sources)?;
        let (core_classes, _) = hierarchy(&core.ontology);
        let composed = medulla_server_api::schema_compose::validate_and_compose(sources)?;
        let (classes, parents) = hierarchy(&composed.ontology);
        let categories = classes
            .iter()
            .map(|class| (class.clone(), upper_category(class, &parents).to_string()))
            .collect();
        Ok(PinnedVocabulary {
            revision,
            categories,
            core_classes,
            parents,
        })
    }
}

#[cfg(feature = "shacl")]
pub(super) use compose::pin;
#[cfg(feature = "shacl")]
pub(crate) use compose::pin_composed;

#[cfg(not(feature = "shacl"))]
pub(super) fn pin(
    _request: &medulla_catalog_types::connector_pack::ConnectorPackImportRequest,
    _archive: &[u8],
    _all: &[&PackEntry],
) -> Result<PinnedVocabulary, String> {
    Err("MALFORMED_INDEX: crosswalk and concept_path entries need the shacl feature".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_category_follows_bfo_ancestors() {
        let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        parents.insert("urn:Event".into(), BTreeSet::from(["urn:Act".to_string()]));
        parents.insert("urn:Act".into(), BTreeSet::from([format!("{BFO}0000015")]));
        parents.insert(
            "urn:Person".into(),
            BTreeSet::from([format!("{BFO}0000004")]),
        );
        assert_eq!(upper_category("urn:Event", &parents), "process");
        assert_eq!(upper_category("urn:Person", &parents), "continuant");
        assert_eq!(upper_category("urn:Other", &parents), "unclassified");
    }
}
