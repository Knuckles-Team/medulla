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
}

impl HeldDocument {
    pub fn content_digest(&self) -> String {
        format!("sha256:{}", self.digest_hex)
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
    Ok(HeldDocument {
        digest_hex: hex::encode(Sha256::digest(text.as_bytes())),
        text,
        backend,
        citation,
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
    };
    Ok((write_set, receipt))
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

pub fn query_digest(query: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(query.trim().as_bytes()))
    )
}
