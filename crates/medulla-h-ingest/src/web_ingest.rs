//! Web page ingest: Document enrichment (MEDULLA-UNIFIED-DATA-PLANE-R651.7) and
//! page selection (MEDULLA-DECISION-ENGINE-R802).
//!
//! Enrichment chunks one held Document with `medulla_document::processing`,
//! embeds the chunks, and lowers chunk nodes, `chunkOf` edges, vectors and one
//! `DocumentEnrichment` record into ONE write-set that commits through the
//! same ChangeEnvelope authority as `IndexRepository` -- no second graph
//! writer. The record id is keyed by the Document and its content digest, so
//! an enriched digest replays without writing, and a new digest removes the
//! earlier digest's chunks in the same commit.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::connector_pack::ConnectorSchemaMapping;
use medulla_decision_outcome_types::decision::ingestion_ladder::ProvenanceRung;
use medulla_decision_types::decision::ingestion_admission::{
    classify_unit, AdmissionKnobs, ContentClass,
};
use medulla_document::processing::{chunk_text, ChunkSpan, ChunkingConfig, CHUNKER_VERSION};
use medulla_source_types::{
    source_ingestion::web_document::AdmissionOutcome,
    source_ingestion::web_document::EnrichDocumentReceipt,
    source_ingestion::web_document::EnrichDocumentRequest,
    source_ingestion::web_document::FetchBackend, source_ingestion::web_document::UnitAdmission,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::ingestion_admission::{admit_unit, exact_digest};
use super::repository_index::{stored_value, EnvelopeLabels, IndexWriteSet};
use medulla_protocol_types::protocol::Method;

pub const ENRICHMENT_TYPE: &str = "DocumentEnrichment";
pub const CHUNK_TYPE: &str = "DocumentChunk";
pub const CHUNK_OF: &str = "chunkOf";
pub const HAS_ENRICHMENT: &str = "hasEnrichment";
const ENVELOPE_PREFIX: &str = "document-enrichment:";
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "document-enrichment-sha256",
    policy_version: "document-enrichment",
};
/// Document properties copied onto every chunk so a citation needs no join.
const CITATION_FIELDS: &[&str] = &["url", "canonical_url", "retrieved_at", "title"];

/// What enrichment read from the held Document.
pub struct HeldDocument {
    pub text: String,
    pub digest_hex: String,
    pub backend: Option<FetchBackend>,
    pub citation: Map<String, Value>,
    /// The stored media type, lowercased and without parameters.
    pub media_type: Option<String>,
}

impl HeldDocument {
    pub fn content_digest(&self) -> String {
        format!("sha256:{}", self.digest_hex)
    }

    /// Whether the Document is a JSON entity document that lowers to alignment
    /// claims rather than chunks (MEDULLA-DECISION-ENGINE-R099.2).
    pub fn is_entity_document(&self) -> bool {
        matches!(
            self.media_type.as_deref(),
            Some("application/json" | "application/ld+json")
        )
    }
}

/// Read the text, digest, backend and citation fields of a Document's stored
/// properties. Refuses a Document with no text or an unknown backend.
pub fn held_document(
    request: &EnrichDocumentRequest,
    properties: &[u8],
) -> Result<HeldDocument, String> {
    let value = medulla_types::msgpack::decode_property_value(properties)
        .map_err(|_| "DOCUMENT_PROPERTIES_INVALID: the Document properties do not decode")?;
    let candidates: Vec<&str> = match request.text_property.as_deref() {
        Some(name) => vec![name],
        None => vec!["text", "content", "body"],
    };
    let text = candidates
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .ok_or("DOCUMENT_TEXT_MISSING: the Document holds no text to enrich")?
        .to_string();
    let stored = ["fetch_backend", "backend"]
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str));
    let backend = match (request.fetch_backend, stored) {
        (Some(backend), _) => Some(backend),
        (None, Some(name)) => Some(FetchBackend::parse(name).ok_or_else(|| {
            format!("WEB_BACKEND_INVALID: '{name}' is not webarchiver or archivebox")
        })?),
        (None, None) => None,
    };
    let citation = CITATION_FIELDS
        .iter()
        .filter_map(|name| Some((name.to_string(), value.get(*name)?.clone())))
        .collect();
    let media_type = ["media_type", "content_type"]
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .and_then(|raw| raw.split(';').next())
        .map(|media| media.trim().to_ascii_lowercase())
        .filter(|media| !media.is_empty());
    Ok(HeldDocument {
        digest_hex: hex::encode(Sha256::digest(text.as_bytes())),
        text,
        backend,
        citation,
        media_type,
    })
}

/// The chunking config a request asks for.
pub fn chunking(request: &EnrichDocumentRequest) -> Result<ChunkingConfig, String> {
    let default = ChunkingConfig::default();
    let config = ChunkingConfig {
        max_bytes: request
            .max_chunk_bytes
            .map_or(default.max_bytes, |b| b as usize),
        overlap_bytes: request
            .overlap_bytes
            .map_or(default.overlap_bytes, |b| b as usize),
    };
    config.validate()?;
    Ok(config)
}

pub fn enrichment_id(document_id: &str, digest_hex: &str) -> String {
    format!("enrichment:{document_id}:{}", &digest_hex[..32])
}

fn chunk_id(document_id: &str, digest_hex: &str, ordinal: u32) -> String {
    format!("chunk:{document_id}:{}:{ordinal}", &digest_hex[..16])
}

/// The ladder rung the embedding producer writes under
/// (MEDULLA-DECISION-ENGINE-R080.2).
pub const PRODUCING_RUNG: ProvenanceRung = ProvenanceRung::Embeddings;

/// The lowest rung among the Document's stored enrichment records that
/// outranks this producer, if any. The embedding candidate then leaves the
/// stored lower-rung fact in place and the call is refused
/// (MEDULLA-DECISION-ENGINE-R080.2): `resolve_rung` decides against the stored
/// record on the same subject and property.
pub fn outranking_rung(document_id: &str, records: &[(String, Vec<u8>)]) -> Option<ProvenanceRung> {
    records
        .iter()
        .filter_map(|(_, bytes)| medulla_types::msgpack::decode_property_value(bytes).ok())
        .filter(|value| value["document_id"].as_str() == Some(document_id))
        .filter_map(|value| crate::ingestion_rung::stored_rung(&value))
        .filter(|stored| crate::ingestion_rung::outranked(Some(*stored), PRODUCING_RUNG))
        .min()
}

/// An earlier enrichment of the same Document under another digest.
pub struct PriorEnrichment {
    pub enrichment_id: String,
    pub chunk_ids: Vec<String>,
    /// Exact span digest (hex) -> the chunk that holds that span's vector.
    pub embedded: BTreeMap<String, String>,
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The chunk id -> exact span digest map an enrichment record stores.
fn span_digests(record: &Value) -> BTreeMap<String, String> {
    record["span_digests"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(chunk, digest)| Some((chunk.clone(), digest.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Find the Document's enrichment records among `records` (id, properties).
/// Returns the one for `digest_hex`, if any, and every other one.
pub fn prior_enrichments(
    document_id: &str,
    digest_hex: &str,
    records: Vec<(String, Vec<u8>)>,
) -> (Option<Value>, Vec<PriorEnrichment>) {
    let mut current = None;
    let mut prior = Vec::new();
    for (id, bytes) in records {
        let Ok(value) = medulla_types::msgpack::decode_property_value(&bytes) else {
            continue;
        };
        if value["document_id"].as_str() != Some(document_id) {
            continue;
        }
        let Some(old_hex) = value["content_digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
        else {
            continue;
        };
        if old_hex == digest_hex {
            current = Some(value);
            continue;
        }
        prior.push(PriorEnrichment {
            enrichment_id: id,
            chunk_ids: string_list(&value["chunk_ids"]),
            embedded: span_digests(&value)
                .into_iter()
                .map(|(chunk, digest)| (digest, chunk))
                .collect(),
        });
    }
    (current, prior)
}

/// The receipt of an already-enriched digest, read back from its record.
pub fn replay_receipt(
    document_id: &str,
    digest_hex: &str,
    record: &Value,
) -> EnrichDocumentReceipt {
    let chunk_ids = string_list(&record["chunk_ids"]);
    let count = chunk_ids.len() as u32;
    EnrichDocumentReceipt {
        document_id: document_id.to_string(),
        enrichment_id: enrichment_id(document_id, digest_hex),
        content_digest: format!("sha256:{digest_hex}"),
        fetch_backend: record["fetch_backend"]
            .as_str()
            .and_then(FetchBackend::parse),
        embedder: record["embedder"].as_str().unwrap_or_default().to_string(),
        embedding_model: record["embedding_model"].as_str().map(str::to_string),
        dimensions: record["dimensions"].as_u64().unwrap_or(0) as u32,
        chunker: record["chunker"].as_str().unwrap_or_default().to_string(),
        chunk_count: count,
        vector_count: record["vector_count"].as_u64().unwrap_or(0) as u32,
        superseded_chunks: 0,
        replayed: true,
        chunk_ids,
        admissions: serde_json::from_value(record["admissions"].clone()).unwrap_or_default(),
        embedded_units: record["embedded_units"].as_u64().unwrap_or(0) as u32,
        demoted_classes: string_list(&record["demoted_classes"]),
        alignment_claims: Vec::new(),
    }
}

/// Everything one enrichment writes.
pub struct EnrichmentPlan<'a> {
    pub document_id: &'a str,
    pub document: &'a HeldDocument,
    pub config: ChunkingConfig,
    pub spans: &'a [ChunkSpan],
    pub vectors: Vec<Vec<f32>>,
    pub embedder: &'static str,
    pub embedding_model: Option<String>,
    pub dimensions: usize,
    pub prior: Vec<PriorEnrichment>,
    /// One verdict per chunked span, in span order (R079.2).
    pub admissions: Vec<UnitAdmission>,
    /// Exact span digest (hex) of each kept span, parallel to `spans`.
    pub digests: Vec<String>,
    /// How many texts were sent to the embedder.
    pub embedded_units: u32,
    /// The content classes retrieval feedback had demoted for this call
    /// (R079.5), snake_case labels.
    pub demoted_classes: Vec<String>,
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| format!("enrichment encoding failed: {error}"))
}

fn node(id: String, properties: Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: encode(&properties)?,
    })
}

fn edge(source: &str, target: &str, relationship: &str) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source.to_string(),
        target_id: target.to_string(),
        properties_msgpack: encode(&json!({ "relationship": relationship }))?,
    })
}

/// The chunk node's properties: offsets, section, text and provenance.
fn chunk_properties(plan: &EnrichmentPlan<'_>, span: &ChunkSpan) -> Value {
    let heading = span.heading_path.join(" > ");
    let name = match (heading.is_empty(), plan.document.citation.get("title")) {
        (false, _) => format!("{heading} #{}", span.ordinal),
        (true, Some(Value::String(title))) => format!("{title} #{}", span.ordinal),
        _ => format!("{} #{}", plan.document_id, span.ordinal),
    };
    let mut properties = json!({
        "type": CHUNK_TYPE,
        "name": stored_value(&name),
        "text": stored_value(span.text(&plan.document.text)),
        "document_id": plan.document_id,
        "ordinal": span.ordinal,
        "heading_path": stored_value(&heading),
        "start_char": span.start_char,
        "end_char": span.end_char,
        "start_byte": span.start_byte,
        "end_byte": span.end_byte,
        "content_digest": plan.document.content_digest(),
        "embedder": plan.embedder,
        "fetch_backend": plan.document.backend.map(FetchBackend::as_str),
    });
    for (key, value) in &plan.document.citation {
        let value = match value {
            Value::String(text) => Value::String(stored_value(text).to_string()),
            other => other.clone(),
        };
        properties[key] = value;
    }
    properties
}

/// Lower a plan to its digest-keyed write-set and its receipt.
pub fn lower(plan: EnrichmentPlan<'_>) -> Result<(IndexWriteSet, EnrichDocumentReceipt), String> {
    let digest_hex = &plan.document.digest_hex;
    let record_id = enrichment_id(plan.document_id, digest_hex);
    let mut methods = Vec::new();
    let mut superseded = 0;
    for prior in &plan.prior {
        for id in &prior.chunk_ids {
            methods.push(Method::RemoveNode {
                node_id: id.clone(),
            });
            superseded += 1;
        }
        methods.push(Method::RemoveNode {
            node_id: prior.enrichment_id.clone(),
        });
    }
    methods.push(node(
        record_id.clone(),
        json!({
            "type": ENRICHMENT_TYPE,
            "document_id": plan.document_id,
            "content_digest": plan.document.content_digest(),
            "fetch_backend": plan.document.backend.map(FetchBackend::as_str),
            "embedder": plan.embedder,
            "embedding_model": plan.embedding_model,
            "dimensions": plan.dimensions,
            "chunker": CHUNKER_VERSION,
            "max_chunk_bytes": plan.config.max_bytes,
            "overlap_bytes": plan.config.overlap_bytes,
            "chunk_count": plan.spans.len(),
            "vector_count": plan.vectors.len(),
            "chunk_ids": plan
                .spans
                .iter()
                .map(|span| chunk_id(plan.document_id, digest_hex, span.ordinal))
                .collect::<Vec<_>>(),
            "span_digests": plan
                .spans
                .iter()
                .zip(&plan.digests)
                .map(|(span, digest)| (chunk_id(plan.document_id, digest_hex, span.ordinal), digest))
                .collect::<BTreeMap<_, _>>(),
            "admissions": plan.admissions,
            "embedded_units": plan.embedded_units,
            "demoted_classes": plan.demoted_classes,
            (crate::ingestion_rung::PRODUCING_RUNG):
                crate::ingestion_rung::rung_label(PRODUCING_RUNG),
        }),
    )?);
    methods.push(edge(plan.document_id, &record_id, HAS_ENRICHMENT)?);
    let mut chunk_ids = Vec::with_capacity(plan.spans.len());
    for (span, vector) in plan.spans.iter().zip(&plan.vectors) {
        let id = chunk_id(plan.document_id, digest_hex, span.ordinal);
        methods.push(node(id.clone(), chunk_properties(&plan, span))?);
        methods.push(edge(&id, plan.document_id, CHUNK_OF)?);
        methods.push(Method::AddEmbedding {
            node_id: id.clone(),
            embedding: vector.clone(),
        });
        chunk_ids.push(id);
    }
    let write_set =
        IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/document-enrichment", methods)?;
    let receipt = EnrichDocumentReceipt {
        document_id: plan.document_id.to_string(),
        enrichment_id: record_id,
        content_digest: plan.document.content_digest(),
        fetch_backend: plan.document.backend,
        embedder: plan.embedder.to_string(),
        embedding_model: plan.embedding_model,
        dimensions: plan.dimensions as u32,
        chunker: CHUNKER_VERSION.to_string(),
        chunk_count: plan.spans.len() as u32,
        vector_count: plan.vectors.len() as u32,
        superseded_chunks: superseded,
        replayed: false,
        chunk_ids,
        admissions: plan.admissions,
        embedded_units: plan.embedded_units,
        demoted_classes: plan.demoted_classes,
        alignment_claims: Vec::new(),
    };
    Ok((write_set, receipt))
}

// ── Entity documents become alignment claims (MEDULLA-DECISION-ENGINE-R099.2) ────

/// No mapping matches the entity document's IRI.
pub const ALIGNMENT_NO_MAPPING: &str = "ALIGNMENT_NO_MAPPING";
/// Two local entities claim one key.
pub const ALIGNMENT_AMBIGUOUS: &str = "ALIGNMENT_AMBIGUOUS";
/// The alignment claim node type.
pub const ALIGNMENT_CLAIM_TYPE: &str = "AlignmentClaim";
/// The type of a node standing for an external entity IRI.
pub const EXTERNAL_ENTITY_TYPE: &str = "ExternalEntity";
/// The edge from a local entity to the external entity it is claimed to match.
pub const EXACT_MATCH: &str = "skos:exactMatch";
/// The edge from a local entity to one of its alignment claims.
pub const HAS_ALIGNMENT_CLAIM: &str = "hasAlignmentClaim";
/// The evidence class every alignment fact carries.
pub const ALIGNMENT_EVIDENCE_CLASS: &str = "claim";
const ALIGNMENT_PREFIX: &str = "document-alignment:";
pub const ALIGNMENT_LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "document-alignment-sha256",
    policy_version: "document-alignment",
};
/// Reserved mapping targets: the source field holding the entity IRI, the
/// entity-resolution key, and the exact-match IRIs. Every other target is a
/// property of the local entity that selects which entity the document is about.
const TARGET_IRI: &str = "@id";
const TARGET_KEY: &str = "wikidataId";

/// One visible local node the alignment branch may attach a claim to or find a
/// claim on: a node of the mapping's ontology class, or an `AlignmentClaim`.
pub struct LocalEntity {
    pub node_id: String,
    pub properties: Value,
}

impl LocalEntity {
    /// Decode a stored node row; a row that does not decode is not a candidate.
    pub fn decode(node_id: String, bytes: &[u8]) -> Option<Self> {
        Some(Self {
            node_id,
            properties: medulla_types::msgpack::decode_property_value(bytes).ok()?,
        })
    }
}

fn refusal(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn source_for<'a>(mapping: &'a ConnectorSchemaMapping, target: &str) -> Option<&'a str> {
    mapping
        .fields
        .iter()
        .find(|(_, mapped)| mapped.as_str() == target)
        .map(|(source, _)| source.as_str())
}

fn iri_list(value: Option<&Value>) -> Vec<&str> {
    match value {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn claim_edge(
    source: &str,
    target: &str,
    relationship: &str,
    digest: &str,
) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source.to_string(),
        target_id: target.to_string(),
        properties_msgpack: encode(&json!({
            "relationship": relationship,
            "evidence_class": ALIGNMENT_EVIDENCE_CLASS,
            "content_digest": digest,
        }))?,
    })
}

/// Lower a fetched entity document to alignment claims. The mapping selects the
/// entity IRI pattern (`entity_iri_prefix`), the source fields of the IRI, the
/// key and the exact-match IRIs, and the properties that identify the local
/// entity the document is about. One `wikidataId` claim node and one
/// `skos:exactMatch` edge per exact-match IRI are written for that entity, each
/// with evidence class `claim` and the document `content_digest`. Nothing is
/// merged or removed: entity resolution (R032) decides every merge.
///
/// Refuses `ALIGNMENT_NO_MAPPING` when the mapping does not match the document's
/// IRI and `ALIGNMENT_AMBIGUOUS` when more than one local entity claims the key.
/// A document that matches no local entity writes nothing (`None`).
pub fn lower_alignment(
    document_id: &str,
    document: &HeldDocument,
    mapping: &ConnectorSchemaMapping,
    local: &[LocalEntity],
) -> Result<(Option<IndexWriteSet>, EnrichDocumentReceipt), String> {
    let no_mapping = |detail: &str| refusal(ALIGNMENT_NO_MAPPING, detail);
    let body: Value = serde_json::from_str(&document.text)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| no_mapping("the entity document is not a JSON object"))?;
    let prefix = mapping
        .entity_iri_prefix
        .as_deref()
        .filter(|prefix| !prefix.is_empty())
        .ok_or_else(|| no_mapping("the mapping declares no entity IRI pattern"))?;
    let (Some(iri_field), Some(key_field)) = (
        source_for(mapping, TARGET_IRI),
        source_for(mapping, TARGET_KEY),
    ) else {
        return Err(no_mapping("the mapping names no IRI or key field"));
    };
    let iri = body[iri_field]
        .as_str()
        .ok_or_else(|| no_mapping("the entity document has no IRI"))?;
    let key = body[key_field]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| no_mapping("the entity document has no key"))?;
    if iri.strip_prefix(prefix) != Some(key) {
        return Err(no_mapping(&format!(
            "IRI '{iri}' is not the pattern '{prefix}' followed by key '{key}'"
        )));
    }
    let mut exact: BTreeSet<&str> = BTreeSet::from([iri]);
    if let Some(field) = source_for(mapping, EXACT_MATCH) {
        for target in iri_list(body.get(field)) {
            if !target.contains("://") {
                return Err(no_mapping(&format!(
                    "exact-match target '{target}' is not an IRI"
                )));
            }
            exact.insert(target);
        }
    }
    // The properties that identify the local entity: every other mapped field.
    let identifying: Vec<(&str, &Value)> = mapping
        .fields
        .iter()
        .filter(|(_, target)| !matches!(target.as_str(), TARGET_IRI | TARGET_KEY | EXACT_MATCH))
        .filter_map(|(source, target)| Some((target.as_str(), body.get(source)?)))
        .filter(|(_, value)| !value.is_null())
        .collect();
    if identifying.is_empty() {
        return Err(no_mapping(
            "the mapping or document carries no property that identifies a local entity",
        ));
    }
    let class = mapping.ontology_class.as_str();
    let is_class = |entity: &LocalEntity| entity.properties["type"].as_str() == Some(class);
    let candidates: BTreeSet<&str> = local
        .iter()
        .filter(|entity| is_class(entity))
        .filter(|entity| {
            identifying
                .iter()
                .all(|(property, value)| entity.properties.get(*property) == Some(*value))
        })
        .map(|entity| entity.node_id.as_str())
        .collect();
    // Entities that already hold this key: by their own property, or by a claim.
    let mut claimants = candidates.clone();
    for entity in local {
        if entity.properties[TARGET_KEY].as_str() != Some(key) {
            continue;
        }
        if is_class(entity) {
            claimants.insert(entity.node_id.as_str());
        } else if entity.properties["type"].as_str() == Some(ALIGNMENT_CLAIM_TYPE) {
            if let Some(held) = entity.properties["entity_id"].as_str() {
                claimants.insert(held);
            }
        }
    }
    if claimants.len() > 1 {
        return Err(refusal(
            ALIGNMENT_AMBIGUOUS,
            format!(
                "{} local entities claim key '{key}': {}",
                claimants.len(),
                claimants.iter().copied().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let digest = document.content_digest();
    let short = &document.digest_hex[..16];
    let mut receipt = EnrichDocumentReceipt {
        document_id: document_id.to_string(),
        enrichment_id: format!("alignment:{document_id}:{short}"),
        content_digest: digest.clone(),
        fetch_backend: document.backend,
        embedder: "none".to_string(),
        embedding_model: None,
        dimensions: 0,
        chunker: "alignment".to_string(),
        chunk_count: 0,
        vector_count: 0,
        superseded_chunks: 0,
        replayed: false,
        chunk_ids: Vec::new(),
        admissions: Vec::new(),
        embedded_units: 0,
        demoted_classes: Vec::new(),
        alignment_claims: Vec::new(),
    };
    let Some(entity) = candidates.iter().next().copied() else {
        return Ok((None, receipt));
    };
    let claim_id = format!("alignment-claim:{entity}:{short}");
    let mut methods = vec![node(
        claim_id.clone(),
        json!({
            "type": ALIGNMENT_CLAIM_TYPE,
            "entity_id": entity,
            "wikidataId": key,
            "document_id": document_id,
            "evidence_class": ALIGNMENT_EVIDENCE_CLASS,
            "content_digest": digest,
        }),
    )?];
    methods.push(claim_edge(entity, &claim_id, HAS_ALIGNMENT_CLAIM, &digest)?);
    for target in exact {
        let external = format!("external-entity:{target}");
        methods.push(node(
            external.clone(),
            json!({ "type": EXTERNAL_ENTITY_TYPE, "iri": target }),
        )?);
        methods.push(claim_edge(entity, &external, EXACT_MATCH, &digest)?);
    }
    let write_set =
        IndexWriteSet::from_methods(ALIGNMENT_PREFIX, b"eg/document-alignment", methods)?;
    receipt.alignment_claims = vec![claim_id];
    Ok((Some(write_set), receipt))
}

/// The source path or URL a held Document came from, for content-class
/// classification: its canonical URL, else its URL, else empty.
pub fn path_hint(document: &HeldDocument) -> String {
    ["canonical_url", "url"]
        .iter()
        .find_map(|key| document.citation.get(*key)?.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One chunked span with its admission verdict and exact digest.
pub struct DecidedSpan {
    pub span: ChunkSpan,
    pub admission: UnitAdmission,
    pub digest: String,
}

impl DecidedSpan {
    /// Whether the span becomes a chunk node.
    pub fn kept(&self) -> bool {
        matches!(
            self.admission.outcome,
            AdmissionOutcome::Admitted | AdmissionOutcome::SkippedExact
        )
    }
}

/// Decide every span of `document` (MEDULLA-DECISION-ENGINE-R079.2, R079.3):
/// the document-level class (lockfile, vendored, generated) applies to every
/// span; a span is also classified on its own text. `known_exact` is the set
/// of exact digests whose vectors are available for reuse; `demoted` holds the
/// classes retrieval feedback has demoted (R079.5).
pub fn decide_spans(
    document: &HeldDocument,
    spans: Vec<ChunkSpan>,
    knobs: &AdmissionKnobs,
    known_exact: &BTreeSet<String>,
    demoted: &BTreeSet<ContentClass>,
) -> Vec<DecidedSpan> {
    let hint = path_hint(document);
    let document_class = classify_unit(&hint, &document.text, knobs);
    let mut seen = BTreeSet::new();
    spans
        .into_iter()
        .map(|span| {
            let text = span.text(&document.text);
            let class = if document_class.admits() {
                classify_unit(&hint, text, knobs)
            } else {
                document_class
            };
            let admission = admit_unit(
                span.ordinal,
                class,
                text,
                known_exact,
                demoted,
                &mut seen,
                knobs,
            );
            DecidedSpan {
                span,
                admission,
                digest: exact_digest(text),
            }
        })
        .collect()
}

/// The vectors of the kept spans, in order: an admitted span takes the next
/// freshly embedded vector, a skipped-exact span reuses its prior vector.
pub fn kept_vectors(
    decided: &[DecidedSpan],
    mut embedded: Vec<Vec<f32>>,
    reusable: &BTreeMap<String, Vec<f32>>,
) -> Result<Vec<Vec<f32>>, String> {
    embedded.reverse();
    let mut out = Vec::new();
    for unit in decided.iter().filter(|u| u.kept()) {
        let vector = match unit.admission.outcome {
            AdmissionOutcome::SkippedExact => reusable.get(&unit.digest).cloned(),
            _ => embedded.pop(),
        };
        out.push(vector.ok_or("ENRICHMENT_VECTOR_MISSING: a kept span has no vector")?);
    }
    Ok(out)
}

/// Chunk a held Document's text under `config`.
pub fn chunks(document: &HeldDocument, config: &ChunkingConfig) -> Result<Vec<ChunkSpan>, String> {
    chunk_text(&document.text, config)
}

// ── Page selection (MEDULLA-DECISION-ENGINE-R802) ────────────────────────────────

/// Unix milliseconds of a stored `retrieved_at`: a number (seconds below
/// 10^11, else milliseconds) or an RFC 3339 UTC timestamp.
pub fn retrieved_at_ms(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return Some(if number < 100_000_000_000 {
            number * 1_000
        } else {
            number
        });
    }
    let text = value.as_str()?;
    let date: Vec<i64> = text
        .get(..10)?
        .split('-')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let time: Vec<i64> = text
        .get(11..19)?
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if date.len() != 3 || time.len() != 3 {
        return None;
    }
    let (y, m, d) = (date[0] - i64::from(date[1] <= 2), date[1], date[2]);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let seconds = days * 86_400 + time[0] * 3_600 + time[1] * 60 + time[2];
    u64::try_from(seconds).ok().map(|s| s * 1_000)
}

/// The request graph's held web pages, keyed by canonical URL, and every
/// content digest it holds. `documents` are RLS-visible `Document` rows.
#[cfg(feature = "decide")]
pub fn held_pages(
    documents: Vec<(String, Vec<u8>)>,
) -> (
    BTreeMap<String, medulla_decision_stats::decision::web_fetch_selection::HeldPage>,
    BTreeSet<String>,
) {
    use medulla_decision_stats::{
        decision::web_fetch_selection::canonical_url, decision::web_fetch_selection::HeldPage,
    };
    let mut held = BTreeMap::new();
    let mut digests = BTreeSet::new();
    for (_, bytes) in documents {
        let Ok(value) = medulla_types::msgpack::decode_property_value(&bytes) else {
            continue;
        };
        let digest = value["content_digest"].as_str().map(str::to_string);
        if let Some(digest) = &digest {
            digests.insert(digest.clone());
        }
        let url = value["canonical_url"]
            .as_str()
            .or_else(|| value["url"].as_str());
        let Some(canonical) = url.and_then(canonical_url) else {
            continue;
        };
        let page = HeldPage {
            retrieved_at_ms: retrieved_at_ms(&value["retrieved_at"])
                .or_else(|| value["retrieved_at_ms"].as_u64()),
            content_digest: digest,
        };
        // Keep the newest copy of a URL.
        let newer = held
            .get(&canonical)
            .is_none_or(|old: &HeldPage| old.retrieved_at_ms < page.retrieved_at_ms);
        if newer {
            held.insert(canonical, page);
        }
    }
    (held, digests)
}

#[cfg(test)]
mod tests;

pub fn query_digest(query: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(query.trim().as_bytes()))
    )
}
