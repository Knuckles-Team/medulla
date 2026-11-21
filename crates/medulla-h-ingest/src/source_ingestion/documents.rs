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

use medulla_compute::sketch::{shingles, LshIndex, MinHashSketch, LSH_MAX_BUCKET_MEMBERS};
use medulla_document::atoms::decompose;
use medulla_document::fragment::{normalize_media_type, ReaderRegistry};
use medulla_document::processing::{chunk_text, ChunkingConfig};
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::ingest_rules::check_atom_provenance;
use medulla_ingestion_types::knowledge_atom::{
    atom_node_id, KnowledgeAtom, ATOM_NODE_TYPE, ATOM_OF,
};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_document::{
    DocumentIngestError, NormalizedDocument, MAX_DOCUMENT_REFS,
};
use medulla_source_types::source_ingestion::{SourceEntityRef, SourceIngestionBatch, SourceRecord};
use medulla_types::contract::Digest256;

use super::ResolvedIngestionMappings;

/// The digest domain of a document identity.
const DOCUMENT_IDENTITY_DOMAIN: &[u8] = b"eg/source-document-identity";

const DOCUMENT_CLASS: &str = medulla_types::ontology_iri!("core#Document");
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// The digest domain of a durable near-duplicate bucket node id.
const LSH_BUCKET_DOMAIN: &[u8] = b"eg/lsh-bucket/v1";

/// The edge from a near duplicate to the earliest similar document
/// (MEDULLA-INGEST-WORKER-R702.4).
pub(super) const NEAR_DUPLICATE_OF: &str = "NEAR_DUPLICATE_OF";

/// Whether a document is stored in full or linked to an earlier near duplicate
/// (MEDULLA-INGEST-WORKER-R702.3).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum DedupOutcome {
    /// No indexed document is at or above the threshold; the document is indexed.
    Original,
    /// The document is at or above the threshold of `of`, an original.
    NearDuplicate { of: String, jaccard: f64 },
}

/// One normalized document with the atoms it decomposed into.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DocumentOutput {
    pub document: NormalizedDocument,
    pub atoms: Vec<KnowledgeAtom>,
    /// The node id of the document's source record.
    pub record_node_id: String,
    /// The MinHash signature of the document text, `bands * rows` minima.
    pub signature: Vec<u64>,
    /// Set by [`dedup_documents`]; `Original` until then.
    pub dedup: DedupOutcome,
    /// The committed graph version an original was first indexed at; set by
    /// [`dedup_documents`].
    pub first_version: u64,
    /// SHA-256 of the blob bytes a reader extracted the text from; `None` for a
    /// record whose text came inline (MEDULLA-INGEST-WORKER-R700.6).
    pub file_sha256: Option<Digest256>,
}

/// The IRIs of `root` and every class that extends it, read from the `rdfs:subClassOf` axioms of
/// `ontologies` (Turtle documents).
pub(crate) fn class_closure(ontologies: &[&str], root: &str) -> Result<BTreeSet<String>, String> {
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
    let mut found = BTreeSet::from([root.to_string()]);
    let mut pending = vec![root.to_string()];
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
    let set = Arc::new(class_closure(&documents, DOCUMENT_CLASS)?);
    *cache = Some((digest, Arc::clone(&set)));
    Ok(set)
}

/// The IRI of a mapping's ontology class: a bare or `:`-prefixed name lives in
/// the core ontology.
pub(crate) fn class_iri(ontology_class: &str) -> String {
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

fn normalization_failed(detail: impl std::fmt::Display) -> DocumentIngestError {
    DocumentIngestError::NormalizationFailed(detail.to_string())
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
    layout: &LshIndex,
    readers: &ReaderRegistry,
    read_blob: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<DocumentOutput>, DocumentIngestError> {
    let mut outputs = Vec::new();
    for record in &batch.records {
        let mapping = resolved
            .entities
            .get(&record.mapping_reference)
            .ok_or_else(|| {
                DocumentIngestError::MappingRevisionMismatch(
                    "UNKNOWN_MAPPING_REFERENCE: record mapping was not resolved".to_string(),
                )
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
            layout,
            readers,
            read_blob,
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
    layout: &LshIndex,
    readers: &ReaderRegistry,
    read_blob: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<DocumentOutput>, DocumentIngestError> {
    let payload = record.payload.value().map_err(normalization_failed)?;
    let source = payload
        .as_object()
        .ok_or_else(|| normalization_failed("source record payload must be an object"))?;
    let value_of = |target: &str| {
        fields
            .iter()
            .find(|(_, mapped)| mapped.as_str() == target)
            .and_then(|(field, _)| source.get(field))
    };
    // A mapped `body` is the text. With no body, a mapped `content_ref` names a blob the
    // engine reads with the reader of the mapped `media_type`; the text is the fragments'
    // text joined in source order (MEDULLA-INGEST-WORKER-R700.3).
    let extracted: String;
    let mut file_sha256: Option<Digest256> = None;
    let text = match value_of("body") {
        Some(serde_json::Value::String(text)) => text.as_str(),
        _ => match value_of("content_ref") {
            Some(serde_json::Value::String(content_ref)) => {
                let media_type = match value_of("media_type") {
                    Some(serde_json::Value::String(media)) if !media.trim().is_empty() => media,
                    _ => {
                        return Err(normalization_failed(format!(
                            "record {} has content_ref without media_type",
                            record.record_id
                        )))
                    }
                };
                let bytes = read_blob(content_ref).map_err(|refusal| {
                    match refusal.split_once(": ") {
                        Some((code, detail)) => DocumentIngestError::from_reader_code(code, detail),
                        None => normalization_failed(refusal),
                    }
                })?;
                file_sha256 = Some(Digest256::sha256(&bytes));
                let fragments = readers
                    .read(&normalize_media_type(media_type), &bytes)
                    .map_err(|error| DocumentIngestError::from_reader_code(error.code, &error.detail))?;
                extracted = fragments
                    .iter()
                    .map(|fragment| fragment.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                extracted.as_str()
            }
            Some(value) if !value.is_null() => return Ok(None),
            _ => "",
        },
    };
    // complete_document: the text is non-empty, at least one atom is emitted
    // and every atom has a provenance key.
    if text.trim().is_empty() {
        return Err(normalization_failed(format!(
            "record {} has no document text",
            record.record_id
        )));
    }
    let raw = rmp_serde::to_vec_named(record)
        .map_err(|error| normalization_failed(format!("record encoding failed: {error}")))?;
    let source_digest = Digest256::sha256(&raw);
    let document_id = content_identity(tenant, connector, record, &source_digest)
        .map_err(normalization_failed)?;
    let chunks = chunk_text(text, &ChunkingConfig::default()).map_err(normalization_failed)?;
    let atoms = decompose(tenant, &document_id, &chunks, text, atom_max_tokens)
        .map_err(normalization_failed)?;
    if atoms.is_empty() {
        return Err(normalization_failed(format!(
            "record {} yields no atom",
            record.record_id
        )));
    }
    if atoms.len() > MAX_DOCUMENT_REFS {
        return Err(normalization_failed(format!(
            "record {} yields more than {MAX_DOCUMENT_REFS} atoms",
            record.record_id
        )));
    }
    for atom in &atoms {
        check_atom_provenance(atom).map_err(|_| {
            DocumentIngestError::MissingProvenance(format!(
                "atom {} has no provenance key",
                atom.atom_id
            ))
        })?;
    }
    let record_node_id = super::source_node_id(
        tenant,
        connector,
        record.stream.as_str(),
        &record.record_id,
    )
    .map_err(normalization_failed)?;
    let signature = document_signature(text, layout).signature().to_vec();
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
        record_node_id,
        signature,
        dedup: DedupOutcome::Original,
        first_version: 0,
        file_sha256,
    }))
}

/// `document_signature`: the MinHash of the 3-word shingles of `text`, of the signature length
/// `layout` needs (MEDULLA-INGEST-WORKER-R702.3).
pub(super) fn document_signature(text: &str, layout: &LshIndex) -> MinHashSketch {
    let mut sketch = layout.new_sketch();
    for shingle in shingles(text) {
        sketch.insert(&shingle);
    }
    sketch
}

/// The band layout of the knobs `ingest.lsh_bands` and `ingest.lsh_rows`.
pub(super) fn lsh_layout() -> Result<LshIndex, String> {
    let knobs = medulla_server_api::knob_config::knobs();
    LshIndex::new(
        knobs.ingest_lsh_bands as usize,
        knobs.ingest_lsh_rows as usize,
    )
    .map_err(|error| error.to_string())
}

/// The node id of the durable bucket of one band key: `lsh-bucket:` plus the hex of the
/// framed digest of tenant, bands, rows, band index and band key, each in decimal.
pub(super) fn bucket_node_id(
    tenant: &str,
    layout: &LshIndex,
    band: usize,
    key: u64,
) -> Result<String, String> {
    let parts = [
        layout.bands().to_string(),
        layout.rows().to_string(),
        band.to_string(),
        key.to_string(),
    ];
    Ok(format!(
        "lsh-bucket:{}",
        Digest256::framed(
            LSH_BUCKET_DOMAIN,
            &[
                tenant.as_bytes(),
                parts[0].as_bytes(),
                parts[1].as_bytes(),
                parts[2].as_bytes(),
                parts[3].as_bytes(),
            ],
        )?
        .to_hex()
    ))
}

fn signature_bucket_ids(
    tenant: &str,
    layout: &LshIndex,
    signature: &[u64],
) -> Result<Vec<String>, String> {
    layout
        .band_keys(signature)
        .into_iter()
        .enumerate()
        .map(|(band, key)| bucket_node_id(tenant, layout, band, key))
        .collect()
}

/// The bucket node ids every document of the batch hashes into: what
/// `read_lsh_candidates` reads before admission.
pub(super) fn candidate_bucket_ids(
    tenant: &str,
    layout: &LshIndex,
    documents: &[DocumentOutput],
) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for document in documents {
        ids.extend(signature_bucket_ids(tenant, layout, &document.signature)?);
    }
    Ok(ids)
}

/// The stored signature of an indexed original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredSignature {
    pub signature: Vec<u64>,
    pub first_version: u64,
}

/// What the graph holds about the buckets a batch hashes into.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LshCandidates {
    /// Bucket node id to the record node ids it holds, in insertion order.
    pub buckets: BTreeMap<String, Vec<String>>,
    /// Record node id to its stored signature.
    pub members: BTreeMap<String, StoredSignature>,
}

/// The outcome of [`dedup_documents`].
#[derive(Debug, Clone)]
pub(super) struct DedupPlan {
    /// The documents, each with its `dedup` outcome and `first_version`.
    pub documents: Vec<DocumentOutput>,
    /// Bucket nodes of the originals, then one `NEAR_DUPLICATE_OF` edge per near duplicate.
    pub methods: Vec<Method>,
    /// The bucket nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// Near duplicates linked.
    pub duplicates: u64,
}

/// `(atoms_emitted, atoms_refused)` of a deduplicated batch (MEDULLA-INGEST-WORKER-R003.5):
/// the atoms of `Original` documents, which `atom_methods` stores, and the atoms of
/// `NearDuplicate` documents, which are linked to the original and not stored.
pub(super) fn atom_counts(documents: &[DocumentOutput]) -> (u64, u64) {
    documents.iter().fold((0, 0), |(emitted, refused), output| {
        let atoms = output.atoms.len() as u64;
        match output.dedup {
            DedupOutcome::Original => (emitted + atoms, refused),
            DedupOutcome::NearDuplicate { .. } => (emitted, refused + atoms),
        }
    })
}

/// The writes that store one original document's atoms (MEDULLA-INGEST-WORKER-R003.3): per
/// atom one `KnowledgeAtom` node and one `atom_of` edge to the document's record node. A near
/// duplicate stores none (R702.4). The second element is the written node ids, each of which
/// takes one policy record.
pub(super) fn atom_methods(output: &DocumentOutput) -> Result<(Vec<Method>, Vec<String>), String> {
    if output.dedup != DedupOutcome::Original {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut methods = Vec::with_capacity(output.atoms.len() * 2);
    let mut node_ids = Vec::with_capacity(output.atoms.len());
    for atom in &output.atoms {
        let node_id = atom_node_id(&atom.atom_id);
        methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": ATOM_NODE_TYPE,
                "atom_id": atom.atom_id,
                "kind": atom.kind,
                "text": atom.text,
                "provenance_key": atom.provenance_key,
                "tenant": atom.tenant,
            }))?,
        });
        methods.push(Method::AddEdge {
            source_id: node_id.clone(),
            target_id: output.record_node_id.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": ATOM_OF }))?,
        });
        node_ids.push(node_id);
    }
    Ok((methods, node_ids))
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| fail(format!("dedup encoding failed: {error}")))
}

/// `dedup_documents`: decide, in batch order, whether each document is an original or a near
/// duplicate of an earlier original (MEDULLA-INGEST-WORKER-R702.3). One transient index holds
/// the stored `candidates` and each earlier original of the batch. `committed_version` is the
/// graph version the batch commits at.
///
/// `of` is the candidate with the smallest first version, then the earliest batch position,
/// then the smallest node id; a stored candidate sorts before any candidate of the batch. A
/// near duplicate is never indexed, so every link points at an original, and it writes the
/// edge `NEAR_DUPLICATE_OF` with its similarity estimate and the band layout (R702.4, R702.5).
pub(super) fn dedup_documents(
    tenant: &str,
    layout: &LshIndex,
    mut documents: Vec<DocumentOutput>,
    candidates: &LshCandidates,
    threshold: f64,
    committed_version: u64,
) -> Result<DedupPlan, String> {
    let mut index = LshIndex::new(layout.bands(), layout.rows()).map_err(|e| e.to_string())?;
    // Candidate node id to its signature and its ordering rank (first version, position).
    let mut signatures: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut rank: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    for (id, stored) in &candidates.members {
        if stored.signature.len() != layout.signature_len() {
            continue;
        }
        index.insert(id, &MinHashSketch::from_signature(stored.signature.clone()))?;
        signatures.insert(id.clone(), stored.signature.clone());
        rank.insert(id.clone(), (stored.first_version, 0));
    }
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut edges = Vec::new();
    let mut duplicates = 0u64;
    for (position, document) in documents.iter_mut().enumerate() {
        let own = document.record_node_id.clone();
        let sketch = MinHashSketch::from_signature(document.signature.clone());
        let nearest = index
            .query(&sketch, threshold)
            .into_iter()
            .filter(|id| *id != own)
            .filter_map(|id| rank.get(&id).map(|(version, at)| (*version, *at, id)))
            .min();
        match nearest {
            None => {
                let first_version = candidates
                    .members
                    .get(&own)
                    .map_or(committed_version, |stored| stored.first_version);
                index.insert(&own, &sketch)?;
                signatures.insert(own.clone(), document.signature.clone());
                rank.insert(own.clone(), (first_version, position + 1));
                document.dedup = DedupOutcome::Original;
                document.first_version = first_version;
                for bucket in signature_bucket_ids(tenant, layout, &document.signature)? {
                    let members = buckets.entry(bucket.clone()).or_insert_with(|| {
                        candidates.buckets.get(&bucket).cloned().unwrap_or_default()
                    });
                    if members.len() < LSH_MAX_BUCKET_MEMBERS && !members.contains(&own) {
                        members.push(own.clone());
                    }
                }
            }
            Some((_, _, of)) => {
                let other = signatures
                    .get(&of)
                    .map(|signature| MinHashSketch::from_signature(signature.clone()))
                    .ok_or_else(|| fail("a near-duplicate target has no signature"))?;
                let jaccard = sketch.jaccard(&other);
                edges.push(Method::AddEdge {
                    source_id: own,
                    target_id: of.clone(),
                    properties_msgpack: encode(&serde_json::json!({
                        "relationship": NEAR_DUPLICATE_OF,
                        "similarity_estimate": jaccard,
                        "lsh_bands": layout.bands(),
                        "lsh_rows": layout.rows(),
                    }))?,
                });
                document.dedup = DedupOutcome::NearDuplicate { of, jaccard };
                duplicates += 1;
            }
        }
    }
    let mut methods = Vec::with_capacity(buckets.len() + edges.len());
    let mut node_ids = Vec::with_capacity(buckets.len());
    for (bucket, members) in &buckets {
        // A bucket that gained no member is not rewritten.
        if candidates.buckets.get(bucket) == Some(members) {
            continue;
        }
        methods.push(Method::AddNode {
            node_id: bucket.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": "LshBucket",
                "members": members,
            }))?,
        });
        node_ids.push(bucket.clone());
    }
    methods.extend(edges);
    Ok(DedupPlan {
        documents,
        methods,
        node_ids,
        duplicates,
    })
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

    fn layout() -> LshIndex {
        LshIndex::new(20, 5).unwrap()
    }

    fn readers() -> ReaderRegistry {
        medulla_document::readers::text_registry(
            medulla_document::readers::ReaderLimits::new(1 << 20, 2).unwrap(),
        )
        .unwrap()
    }

    fn no_blob(_: &str) -> Result<Vec<u8>, String> {
        Err("BODY_MISSING: no blob is held".to_string())
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
        let first = normalize_record("t1", "demo", &rec, &fields(), 4, 200, &layout(), &readers(), &no_blob)
            .unwrap()
            .unwrap();
        let second = normalize_record("t1", "demo", &rec, &fields(), 4, 200, &layout(), &readers(), &no_blob)
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
        let other_tenant = normalize_record("t2", "demo", &rec, &fields(), 4, 200, &layout(), &readers(), &no_blob)
            .unwrap()
            .unwrap();
        assert_ne!(
            first.document.document_id,
            other_tenant.document.document_id
        );
        let changed = record(serde_json::json!({"text": "A different body."}));
        let other = normalize_record("t1", "demo", &changed, &fields(), 4, 200, &layout(), &readers(), &no_blob)
            .unwrap()
            .unwrap();
        assert_ne!(first.document.document_id, other.document.document_id);

        // complete_document refuses empty text and a record with no body.
        for payload in [
            serde_json::json!({"text": "   "}),
            serde_json::json!({"other": "x"}),
        ] {
            let error = normalize_record(
                "t1",
                "demo",
                &record(payload),
                &fields(),
                4,
                200,
                &layout(),
                &readers(),
                &no_blob,
            )
            .unwrap_err();
            let error = error.to_string();
            assert!(
                error.starts_with("DOCUMENT_NORMALIZATION_FAILED:"),
                "{error}"
            );
        }
        // A blob-backed document with no media type fails.
        let blob_fields = BTreeMap::from([("blob".to_string(), "content_ref".to_string())]);
        let blob = record(serde_json::json!({"blob": "cas:sha256:ab"}));
        let error = normalize_record("t1", "demo", &blob, &blob_fields, 4, 200, &layout(), &readers(), &no_blob)
            .unwrap_err()
            .to_string();
        assert!(error.contains("has content_ref without media_type"), "{error}");
    }

    // spec: MEDULLA-INGEST-WORKER-R700.3
    #[test]
    fn a_content_ref_record_reads_through_its_media_type_reader() {
        let blob_fields = BTreeMap::from([
            ("blob".to_string(), "content_ref".to_string()),
            ("type".to_string(), "media_type".to_string()),
        ]);
        let csv = b"name,role\nAda,engineer\nGrace,admiral\n".to_vec();
        let read_csv = |content_ref: &str| -> Result<Vec<u8>, String> {
            assert_eq!(content_ref, "cas:sha256:aa");
            Ok(csv.clone())
        };
        let rec = record(serde_json::json!({"blob": "cas:sha256:aa", "type": "text/csv; charset=utf-8"}));
        let output = normalize_record("t1", "demo", &rec, &blob_fields, 4, 200, &layout(), &readers(), &read_csv)
            .unwrap()
            .expect("the reader yields a document");
        assert!(!output.atoms.is_empty(), "at least one atom per document");
        assert_eq!(output.document.atom_ids.len(), output.atoms.len());
        // An unreadable blob and an unknown media type fail the record.
        let missing = |_: &str| -> Result<Vec<u8>, String> { Err("BODY_MISSING: absent".into()) };
        let error = normalize_record("t1", "demo", &rec, &blob_fields, 4, 200, &layout(), &readers(), &missing)
            .unwrap_err()
            .to_string();
        assert!(error.contains("BODY_MISSING"), "{error}");
        let odd = record(serde_json::json!({"blob": "cas:sha256:aa", "type": "application/x-unknown"}));
        let error = normalize_record("t1", "demo", &odd, &blob_fields, 4, 200, &layout(), &readers(), &read_csv)
            .unwrap_err()
            .to_string();
        assert!(error.contains("READER_UNSUPPORTED_MEDIA_TYPE"), "{error}");
    }

    // spec: MEDULLA-INGEST-WORKER-R003.10
    #[test]
    fn each_error_maps_to_one_quarantine_reason() {
        use medulla_source_types::source_document::QuarantineReason;
        let cases = [
            (
                DocumentIngestError::NormalizationFailed("no text".into()),
                QuarantineReason::NormalizationFailed,
                "DOCUMENT_NORMALIZATION_FAILED",
            ),
            (
                DocumentIngestError::from_reader_code("READER_TOO_LARGE", "9 bytes exceed 8"),
                QuarantineReason::NormalizationFailed,
                "READER_TOO_LARGE",
            ),
            (
                DocumentIngestError::MissingProvenance("atom a1".into()),
                QuarantineReason::MissingProvenance,
                "ATOM_PROVENANCE_MISSING",
            ),
            (
                DocumentIngestError::MappingRevisionMismatch("rev 3".into()),
                QuarantineReason::MappingRevisionMismatch,
                "MAPPING_REVISION_MISMATCH",
            ),
        ];
        for (error, reason, rule) in cases {
            assert_eq!(error.quarantine_reason(), reason);
            assert_eq!(error.rule_id(), rule);
            assert!(error.to_string().starts_with(rule), "{error}");
        }
        // A record that yields no text is a normalization failure.
        let error = normalize_record(
            "t1",
            "demo",
            &record(serde_json::json!({"text": "  "})),
            &fields(),
            4,
            200,
            &layout(),
            &readers(),
            &no_blob,
        )
        .unwrap_err();
        assert_eq!(error.quarantine_reason(), QuarantineReason::NormalizationFailed);
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
        let set = class_closure(&[&turtle], DOCUMENT_CLASS).unwrap();
        for name in ["Document", "Note", "Memo"] {
            assert!(set.contains(&class_iri(&format!(":{name}"))), "{name}");
        }
        assert!(!set.contains(&class_iri("Person")));
        assert_eq!(class_iri("https://x.test/o#Doc"), "https://x.test/o#Doc");
    }

    fn document(id: &str, text: &str) -> DocumentOutput {
        let mut rec = record(serde_json::json!({ "text": text }));
        rec.record_id = id.into();
        normalize_record("t1", "demo", &rec, &fields(), 4, 200, &layout(), &readers(), &no_blob)
            .unwrap()
            .unwrap()
    }

    fn words(prefix: &str, last: &str) -> String {
        let mut words: Vec<String> = (0..59).map(|i| format!("{prefix}{i}")).collect();
        words.push(last.to_string());
        format!("{}.", words.join(" "))
    }

    // spec: MEDULLA-INGEST-WORKER-R702.3
    #[test]
    fn dedup_runs_before_atom_storage() {
        let layout = layout();
        let first = document("doc-a", &words("w", "end"));
        let near = document("doc-b", &words("w", "fin"));
        let distinct = document("doc-c", &words("x", "end"));
        assert_eq!(first.signature.len(), layout.signature_len());

        // Nothing stored: the first and the distinct document are originals, the second is
        // linked to the first, and every outcome is known before any record is admitted.
        let plan = dedup_documents(
            "t1",
            &layout,
            vec![first.clone(), near.clone(), distinct.clone()],
            &LshCandidates::default(),
            0.8,
            7,
        )
        .unwrap();
        assert_eq!(plan.documents[0].dedup, DedupOutcome::Original);
        assert_eq!(plan.documents[0].first_version, 7);
        match &plan.documents[1].dedup {
            DedupOutcome::NearDuplicate { of, jaccard } => {
                assert_eq!(of, &first.record_node_id);
                assert!(*jaccard >= 0.8, "{jaccard}");
            }
            other => panic!("expected a near duplicate, got {other:?}"),
        }
        assert_eq!(plan.documents[2].dedup, DedupOutcome::Original);
        assert_eq!(plan.duplicates, 1);
        let nodes = plan
            .methods
            .iter()
            .filter(|method| matches!(method, Method::AddNode { .. }))
            .count();
        let edges = plan
            .methods
            .iter()
            .filter(|method| matches!(method, Method::AddEdge { .. }))
            .count();
        assert_eq!(edges, 1);
        assert_eq!(nodes, plan.node_ids.len());
        assert!(nodes > 0 && nodes <= 2 * layout.bands());
        assert!(plan.node_ids.iter().all(|id| id.starts_with("lsh-bucket:")));
        // The edge is the last method, after the bucket nodes.
        match plan.methods.last() {
            Some(Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            }) => {
                assert_eq!(source_id, &near.record_node_id);
                assert_eq!(target_id, &first.record_node_id);
                let props: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
                assert_eq!(props["relationship"], NEAR_DUPLICATE_OF);
                assert_eq!(props["lsh_bands"], 20);
                assert_eq!(props["lsh_rows"], 5);
                assert!(props["similarity_estimate"].as_f64().unwrap() >= 0.8);
            }
            other => panic!("expected the near-duplicate edge last, got {other:?}"),
        }

        // A stored original is read from its buckets and wins over the batch: the second
        // document links to it and writes no bucket node.
        let mut stored = LshCandidates::default();
        stored.members.insert(
            first.record_node_id.clone(),
            StoredSignature {
                signature: first.signature.clone(),
                first_version: 3,
            },
        );
        for id in candidate_bucket_ids("t1", &layout, std::slice::from_ref(&first)).unwrap() {
            stored.buckets.insert(id, vec![first.record_node_id.clone()]);
        }
        let plan = dedup_documents("t1", &layout, vec![near.clone()], &stored, 0.8, 9).unwrap();
        assert!(matches!(
            &plan.documents[0].dedup,
            DedupOutcome::NearDuplicate { of, .. } if *of == first.record_node_id
        ));
        assert!(plan.node_ids.is_empty());
        assert_eq!(plan.methods.len(), 1);

        // A re-ingested stored original is not its own near duplicate and keeps its version.
        let plan = dedup_documents("t1", &layout, vec![first.clone()], &stored, 0.8, 9).unwrap();
        assert_eq!(plan.documents[0].dedup, DedupOutcome::Original);
        assert_eq!(plan.documents[0].first_version, 3);
        assert!(plan.methods.is_empty());
    }

    // spec: MEDULLA-INGEST-WORKER-R003.3
    #[test]
    fn original_documents_store_atoms_and_near_duplicates_store_none() {
        let mut original = document("doc-a", &words("w", "end"));
        let (methods, node_ids) = atom_methods(&original).unwrap();
        assert_eq!(node_ids.len(), original.atoms.len());
        assert_eq!(methods.len(), 2 * original.atoms.len());
        match &methods[1] {
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => {
                assert_eq!(source_id, &node_ids[0]);
                assert_eq!(target_id, &original.record_node_id);
                let edge: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
                assert_eq!(edge["relationship"], ATOM_OF);
            }
            _ => panic!("expected the atom_of edge"),
        }
        original.dedup = DedupOutcome::NearDuplicate {
            of: "other".into(),
            jaccard: 0.9,
        };
        let (methods, node_ids) = atom_methods(&original).unwrap();
        assert!(methods.is_empty() && node_ids.is_empty());
    }

    // spec: MEDULLA-INGEST-WORKER-R003.9
    #[test]
    fn prepare_validates_document_after_resolution() {
        // `prepare` runs, in this order: authorization, the committed-replay shortcut, one
        // mapping resolution, document normalization, reconciliation, record admission.
        let source = include_str!("../source_ingestion.rs");
        let gate = "#[cfg(all(feature = \"redb\", feature = \"blob\"))]\npub async fn prepare(";
        let start = source.find(gate).expect("the served prepare exists");
        let body = &source[start..];
        let end = body.find("\n}\n").expect("prepare ends");
        let body = &body[..end];
        let at = |needle: &str| {
            let hits: Vec<usize> = body.match_indices(needle).map(|(at, _)| at).collect();
            assert_eq!(hits.len(), 1, "{needle} is called once in prepare");
            hits[0]
        };
        let order = [
            at("authorize_ingestion(&ctx)"),
            at("read_change_envelope("),
            at("resolve_mappings(&ctx, batch, &resources)"),
            at("documents::normalize_documents("),
            at("reconcile_batch("),
            at("admit_records("),
        ];
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "prepare calls authorization, replay, resolution, normalization, reconciliation and admission in order: {order:?}"
        );
    }

    /// A quarantine prepared for the fixture batch, with the stream's prior checkpoint.
    fn quarantine_fixture() -> (
        medulla_source_types::source_ingestion::SourceIngestionBatch,
        Digest256,
        String,
        crate::source_ingestion::PreparedSourceIngestion,
    ) {
        use medulla_source_types::source_ingestion::SourceIngestionRequest;
        let batch = medulla_source_types::source_ingestion::fixtures::request()
            .as_batch()
            .clone();
        let digest = SourceIngestionRequest::new(batch.clone())
            .unwrap()
            .batch_digest()
            .unwrap();
        let batch_id = format!("source-ingest:{}", digest.to_hex());
        let verified = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
            "agent:ingest",
            "t",
        );
        let scope = medulla_server_api::batch_compile::GraphWriteScope {
            request_id: 1,
            graph_name: "g",
            tenant_scope: "t",
            verified: &verified,
            graph_version: 4,
            placement_epoch: 0,
            fencing_token: None,
        };
        let prepared = crate::source_ingestion::quarantine_prepared(
            scope,
            crate::source_ingestion::QuarantineInputs {
                batch: &batch,
                batch_digest: digest,
                batch_id: &batch_id,
                error: &DocumentIngestError::NormalizationFailed("no text".into()),
                mapping_revision: 3,
                stream_object_id: crate::source_ingestion::source_marker_id("t", &batch).unwrap(),
                previous_stream_digest: Some("prior-digest".into()),
                prior_accepted_checkpoint: Some(batch.provider_checkpoint.clone()),
                committed_graph_version: 5,
            },
        )
        .unwrap();
        (batch, digest, batch_id, prepared)
    }

    // spec: MEDULLA-INGEST-WORKER-R003.10.1
    #[test]
    fn invalid_document_quarantines_without_partial_graph() {
        let (batch, digest, batch_id, prepared) = quarantine_fixture();
        let envelope = &prepared.envelope;
        // One node upsert holds the restricted evidence and nothing else.
        assert_eq!(envelope.mutation.operations.len(), 1);
        let Method::AddNode {
            node_id,
            properties_msgpack,
        } = &envelope.mutation.operations[0].method
        else {
            panic!("the quarantine writes one node");
        };
        assert_eq!(
            node_id,
            &crate::source_ingestion::quarantine_node_id(
                "t",
                batch.connector.as_str(),
                batch.provider_checkpoint.stream.as_str(),
                &batch_id
            )
            .unwrap()
        );
        let stored: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
        assert_eq!(stored["type"], "SourceQuarantine");
        assert_eq!(stored["reason"], "normalization_failed");
        assert_eq!(stored["rule_id"], "DOCUMENT_NORMALIZATION_FAILED");
        assert_eq!(stored["source_digest"], digest.to_hex());
        assert_eq!(stored["replay"], "replayable");
        assert_eq!(stored["mapping_revision"], "3");
        // No cursor, feature, blob or lineage moves; the marker chains from its
        // current digest and both governed objects carry a policy.
        assert!(envelope.cursor.is_none());
        assert!(envelope.features.is_empty());
        assert!(envelope.blobs.is_empty());
        assert!(envelope.lineage.is_empty());
        assert_eq!(envelope.policies.len(), 2);
        assert_eq!(
            envelope.content_version.previous_digest.as_deref(),
            Some("prior-digest")
        );
        // No accepted projection, and the prior accepted checkpoint is kept.
        assert!(prepared.raw_admissions.is_empty());
        assert!(prepared.mappings.is_empty());
        assert_eq!(prepared.affected_count(), 0);
        let receipt = prepared.quarantine.as_ref().expect("a quarantine receipt");
        assert_eq!(receipt.restricted_evidence_ref, format!("quarantine:{node_id}"));
        assert_eq!(
            receipt.prior_accepted_checkpoint.as_ref(),
            Some(&batch.provider_checkpoint)
        );
        assert!(
            !stored.to_string().contains("no text"),
            "the failure detail and source text are never stored"
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R003.11
    #[test]
    fn quarantine_finish_and_replay_preserve_outcome() {
        use medulla_protocol_types::change_envelope::ChangeEnvelopeCommit;
        use medulla_protocol_types::protocol::{Response, ResultPayload};
        use medulla_protocol_types::result_contract::transactions::ChangeEnvelopeApplied;
        use medulla_source_types::source_document::SourceIngestionOutcome;
        let (batch, digest, batch_id, prepared) = quarantine_fixture();
        let first_receipt = prepared.quarantine.clone().expect("a quarantine receipt");
        let envelope = prepared.envelope.clone();
        let applied = |replayed: bool| {
            let commit = ChangeEnvelopeCommit {
                envelope_id: envelope.envelope_id.clone(),
                batch_id: batch_id.clone(),
                content_version: envelope.content_version.clone(),
                cursor: None,
                outbox_count: 0,
                replayed,
            };
            let body = ChangeEnvelopeApplied {
                commit,
                projection_pending: false,
                replication: None,
                write_conflict: None,
            };
            Response::ok(
                1,
                ResultPayload::Json(serde_json::to_value(body).unwrap()),
            )
        };
        let finished = crate::source_ingestion::finish(prepared, applied(false));
        let Some(ResultPayload::Raw(bytes)) = finished.result else {
            panic!("finish answers a raw outcome: {finished:?}");
        };
        let outcome: SourceIngestionOutcome = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(
            outcome,
            SourceIngestionOutcome::Quarantined(Box::new(first_receipt.clone()))
        );
        // The replay reads the stored evidence node and returns the same receipt,
        // with the prior checkpoint as first stored; it never resets the checkpoint.
        let replayed = crate::source_ingestion::replay_quarantine(
            "t",
            &batch,
            digest,
            &batch_id,
            &envelope,
        )
        .unwrap()
        .expect("the stored envelope is a quarantine");
        assert_eq!(replayed.reason, first_receipt.reason);
        assert_eq!(replayed.rule_id, first_receipt.rule_id);
        assert_eq!(replayed.source_digest, first_receipt.source_digest);
        assert_eq!(replayed.mapping_revision, first_receipt.mapping_revision);
        assert_eq!(
            replayed.restricted_evidence_ref,
            first_receipt.restricted_evidence_ref
        );
        assert_eq!(
            replayed.prior_accepted_checkpoint,
            first_receipt.prior_accepted_checkpoint
        );
        assert_eq!(replayed.committed_graph_version, first_receipt.committed_graph_version);
    }
}
