//! Document normalization and the `complete_document` check
//! (MEDULLA-INGEST-WORKER-R003.8).
//!
//! A source record is a document when its resolved mapping class equals or
//! extends the core class `Document`. Its text is the mapped property `body`;
//! the text is cut into chunks, decomposed into provenance-keyed knowledge
//! atoms and named by a [`NormalizedDocument`]. A record whose text arrives as
//! a blob (`content_ref`) is read by the readers of MEDULLA-INGEST-WORKER-R700
//! and is not normalized here. All class and mapping decisions read the one
//! `ResolvedIngestionMappings` snapshot `prepare` resolved.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use medulla_document::atoms::decompose;
use medulla_document::processing::{chunk_text, ChunkingConfig};
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::ingest_rules::check_atom_provenance;
use medulla_ingestion_types::knowledge_atom::KnowledgeAtom;
use medulla_source_types::source_document::{NormalizedDocument, MAX_DOCUMENT_REFS};
use medulla_source_types::source_ingestion::{SourceEntityRef, SourceIngestionBatch, SourceRecord};
use medulla_types::contract::Digest256;

use super::ResolvedIngestionMappings;

/// The digest domain of a document identity.
const DOCUMENT_IDENTITY_DOMAIN: &[u8] = b"eg/source-document-identity";

const DOCUMENT_CLASS: &str = medulla_types::ontology_iri!("core#Document");
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// One normalized document with the atoms it decomposed into.
#[derive(Debug, Clone, PartialEq, Eq)]
// The atoms are stored by the ingest wiring of MEDULLA-INGEST-WORKER-R003.3.
#[allow(dead_code)]
pub(super) struct DocumentOutput {
    pub document: NormalizedDocument,
    pub atoms: Vec<KnowledgeAtom>,
}

/// The IRIs of `Document` and every class that extends it, read from the
/// `rdfs:subClassOf` axioms of `ontologies` (Turtle documents).
pub(super) fn document_class_set(ontologies: &[&str]) -> Result<BTreeSet<String>, String> {
    use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};
    let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for text in ontologies {
        for triple in medulla_rdf::mapping::parse_turtle(text)? {
            if triple.predicate.as_str() != RDFS_SUBCLASS_OF {
                continue;
            }
            if let (NamedOrBlankNode::NamedNode(sub), Term::NamedNode(sup)) =
                (&triple.subject, &triple.object)
            {
                children
                    .entry(sup.as_str().to_string())
                    .or_default()
                    .insert(sub.as_str().to_string());
            }
        }
    }
    let mut found = BTreeSet::from([DOCUMENT_CLASS.to_string()]);
    let mut pending = vec![DOCUMENT_CLASS.to_string()];
    while let Some(next) = pending.pop() {
        for child in children.get(&next).into_iter().flatten() {
            if found.insert(child.clone()) {
                pending.push(child.clone());
            }
        }
    }
    Ok(found)
}

type Cached = Option<(Digest256, Arc<BTreeSet<String>>)>;

/// The document classes of the graph's composed schema, computed once per
/// schema identity.
pub(super) fn document_classes(
    sources: &GraphSchemaSources,
) -> Result<Arc<BTreeSet<String>>, String> {
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let digest = sources.composed_digest();
    let mut cache = CACHE.lock().map_err(|_| "document class cache poisoned")?;
    if let Some((cached, set)) = cache.as_ref() {
        if *cached == digest {
            return Ok(Arc::clone(set));
        }
    }
    let documents: Vec<&str> = sources.ontologies().map(|(_, text)| text).collect();
    let set = Arc::new(document_class_set(&documents)?);
    *cache = Some((digest, Arc::clone(&set)));
    Ok(set)
}

/// The IRI of a mapping's ontology class: a bare or `:`-prefixed name lives in
/// the core ontology.
fn class_iri(ontology_class: &str) -> String {
    let name = ontology_class.trim().trim_start_matches(':');
    if name.contains("://") {
        name.to_string()
    } else {
        let base = DOCUMENT_CLASS.trim_end_matches("Document");
        format!("{base}{name}")
    }
}

fn fail(detail: impl std::fmt::Display) -> String {
    format!("DOCUMENT_NORMALIZATION_FAILED: {detail}")
}

/// Normalize every document-class record of `batch`. Records of other classes
/// are untouched. The first record that fails `complete_document` fails the
/// batch.
pub(super) fn normalize_documents(
    tenant: &str,
    batch: &SourceIngestionBatch,
    resolved: &ResolvedIngestionMappings,
    classes: &BTreeSet<String>,
    atom_max_tokens: usize,
) -> Result<Vec<DocumentOutput>, String> {
    let mut outputs = Vec::new();
    for record in &batch.records {
        let mapping = resolved
            .entities
            .get(&record.mapping_reference)
            .ok_or_else(|| {
                "UNKNOWN_MAPPING_REFERENCE: record mapping was not resolved".to_string()
            })?;
        if !classes.contains(&class_iri(&mapping.schema_mapping.ontology_class)) {
            continue;
        }
        if let Some(output) = normalize_record(
            tenant,
            batch.connector.as_str(),
            record,
            &mapping.schema_mapping.fields,
            mapping.entry_revision,
            atom_max_tokens,
        )? {
            outputs.push(output);
        }
    }
    Ok(outputs)
}

/// Normalize one document record, or `None` when its text is a blob read by
/// the R700 readers.
pub(super) fn normalize_record(
    tenant: &str,
    connector: &str,
    record: &SourceRecord,
    fields: &BTreeMap<String, String>,
    mapping_revision: u64,
    atom_max_tokens: usize,
) -> Result<Option<DocumentOutput>, String> {
    let payload = record.payload.value()?;
    let source = payload
        .as_object()
        .ok_or_else(|| fail("source record payload must be an object"))?;
    let value_of = |target: &str| {
        fields
            .iter()
            .find(|(_, mapped)| mapped.as_str() == target)
            .and_then(|(field, _)| source.get(field))
    };
    let text = match value_of("body") {
        Some(serde_json::Value::String(text)) => text.as_str(),
        _ if value_of("content_ref").is_some_and(|value| !value.is_null()) => return Ok(None),
        _ => "",
    };
    // complete_document: the text is non-empty, at least one atom is emitted
    // and every atom has a provenance key.
    if text.trim().is_empty() {
        return Err(fail(format!(
            "record {} has no document text",
            record.record_id
        )));
    }
    let raw = rmp_serde::to_vec_named(record)
        .map_err(|error| fail(format!("record encoding failed: {error}")))?;
    let source_digest = Digest256::sha256(&raw);
    let document_id = content_identity(tenant, connector, record, &source_digest)?;
    let chunks = chunk_text(text, &ChunkingConfig::default()).map_err(fail)?;
    let atoms = decompose(tenant, &document_id, &chunks, text, atom_max_tokens).map_err(fail)?;
    if atoms.is_empty() {
        return Err(fail(format!("record {} yields no atom", record.record_id)));
    }
    if atoms.len() > MAX_DOCUMENT_REFS {
        return Err(fail(format!(
            "record {} yields more than {MAX_DOCUMENT_REFS} atoms",
            record.record_id
        )));
    }
    for atom in &atoms {
        check_atom_provenance(atom).map_err(|refusal| refusal.to_string())?;
    }
    Ok(Some(DocumentOutput {
        document: NormalizedDocument {
            document_id,
            source: SourceEntityRef {
                stream: record.stream.clone(),
                record_id: record.record_id.clone(),
            },
            source_digest,
            mapping_revision: mapping_revision.to_string(),
            atom_ids: atoms.iter().map(|atom| atom.atom_id.clone()).collect(),
            concept_ids: Vec::new(),
        },
        atoms,
    }))
}

/// `content_identity`: the document id is SHA-256 framed under
/// `eg/source-document-identity` over tenant, connector, stream, record id and
/// source digest.
pub(super) fn content_identity(
    tenant: &str,
    connector: &str,
    record: &SourceRecord,
    source_digest: &Digest256,
) -> Result<String, String> {
    let digest_hex = source_digest.to_hex();
    Ok(Digest256::framed(
        DOCUMENT_IDENTITY_DOMAIN,
        &[
            tenant.as_bytes(),
            connector.as_bytes(),
            record.stream.as_str().as_bytes(),
            record.record_id.as_bytes(),
            digest_hex.as_bytes(),
        ],
    )?
    .to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_ingestion::{SourceJson, SourceRecordProvenance};
    use medulla_types::contract::ResourceId;

    fn record(payload: serde_json::Value) -> SourceRecord {
        SourceRecord {
            stream: ResourceId::new("docs").unwrap(),
            record_id: "doc-1".into(),
            mapping_reference: "connector:demo#mappings/doc".into(),
            payload: SourceJson::new(payload).unwrap(),
            updated_at: None,
            provenance: SourceRecordProvenance {
                connector: ResourceId::new("demo").unwrap(),
                adapter_kind: ResourceId::new("mcp").unwrap(),
                server: "server".into(),
                tool: "list_docs".into(),
                tool_schema_sha256: Digest256::from_bytes([3; 32]),
                source_uri: "demo:docs:doc-1".into(),
            },
            column_profiles: Default::default(),
        }
    }

    fn fields() -> BTreeMap<String, String> {
        BTreeMap::from([("text".to_string(), "body".to_string())])
    }

    // spec: MEDULLA-INGEST-WORKER-R003.8
    #[test]
    fn complete_document_is_idempotent() {
        let rec = record(serde_json::json!({
            "text": "The engine stores facts in one graph. Always use short reports."
        }));
        let first = normalize_record("t1", "demo", &rec, &fields(), 4, 200)
            .unwrap()
            .unwrap();
        let second = normalize_record("t1", "demo", &rec, &fields(), 4, 200)
            .unwrap()
            .unwrap();
        // A retry yields the same document identity, atoms and receipt.
        assert_eq!(first, second);
        assert_eq!(first.document.document_id.len(), 64);
        assert_eq!(first.document.mapping_revision, "4");
        assert_eq!(first.document.atom_ids.len(), first.atoms.len());
        assert!(first
            .atoms
            .iter()
            .all(|atom| atom.provenance_key.starts_with(&first.document.document_id)));
        // Identity moves with tenant, connector and content.
        let other_tenant = normalize_record("t2", "demo", &rec, &fields(), 4, 200)
            .unwrap()
            .unwrap();
        assert_ne!(
            first.document.document_id,
            other_tenant.document.document_id
        );
        let changed = record(serde_json::json!({"text": "A different body."}));
        let other = normalize_record("t1", "demo", &changed, &fields(), 4, 200)
            .unwrap()
            .unwrap();
        assert_ne!(first.document.document_id, other.document.document_id);

        // complete_document refuses empty text and a record with no body.
        for payload in [
            serde_json::json!({"text": "   "}),
            serde_json::json!({"other": "x"}),
        ] {
            let error =
                normalize_record("t1", "demo", &record(payload), &fields(), 4, 200).unwrap_err();
            assert!(
                error.starts_with("DOCUMENT_NORMALIZATION_FAILED:"),
                "{error}"
            );
        }
        // A blob-backed document is left to the readers.
        let blob_fields = BTreeMap::from([("blob".to_string(), "content_ref".to_string())]);
        let blob = record(serde_json::json!({"blob": "sha256:ab"}));
        assert!(normalize_record("t1", "demo", &blob, &blob_fields, 4, 200)
            .unwrap()
            .is_none());
    }

    #[test]
    fn document_classes_follow_subclass_axioms() {
        let core = DOCUMENT_CLASS.trim_end_matches("Document");
        let turtle = format!(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             <{core}Note> rdfs:subClassOf <{core}Document> .\n\
             <{core}Memo> rdfs:subClassOf <{core}Note> .\n\
             <{core}Person> rdfs:subClassOf <{core}Agent> .\n"
        );
        let set = document_class_set(&[&turtle]).unwrap();
        for name in ["Document", "Note", "Memo"] {
            assert!(set.contains(&class_iri(&format!(":{name}"))), "{name}");
        }
        assert!(!set.contains(&class_iri("Person")));
        assert_eq!(class_iri("https://x.test/o#Doc"), "https://x.test/o#Doc");
    }
}
