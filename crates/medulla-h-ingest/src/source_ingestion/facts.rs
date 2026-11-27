//! Admission of `facts` batches (MEDULLA-INGEST-WORKER-R701.3).
//!
//! The SDK sends only prose facts. The engine grounds each fact against the
//! compiled ontology of the graph (`ground_fact`), drops a repeated triple
//! (`FactDeduper`) and stores every kept fact twice: as a `candidate_claim`
//! node that carries the fact and its grounding, and as a `KnowledgeAtom` of
//! kind `Fact` whose provenance key is the evidence span reference. The atom
//! links to its claim by an `atom_of` edge. A batch that names the chunk it was read
//! from (`source_chunk`, MEDULLA-UNIFIED-DATA-PLANE-R651.7.7) also grounds each claim:
//! an `extractedFrom` edge to the chunk, a `derivedFrom` edge to the chunk's Document,
//! and a `chunk_facts` record that lets a re-extraction supersede the claims it no
//! longer states.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use sha2::{Digest as _, Sha256};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::extracted_fact::{
    CandidateClaim, EvidenceSpan, ExtractedFact, FactBatch, FactChunk, FactOutcome, FactReport,
    FactUnit, GroundingResult,
};
use medulla_ingestion_types::ingest_rules::{
    check_atom_provenance, ground_fact, infer_links, subclass_closure, FactDeduper, GroundingCache,
    RELATED_TO,
};
use medulla_ingestion_types::knowledge_atom::{
    atom_node_id, AtomKind, KnowledgeAtom, ATOM_NODE_TYPE, ATOM_OF,
};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::batch_kind::MAX_FACTS;
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::contract::{Digest256, ServerErrorCode};

/// The `type` property of a stored candidate claim node.
pub const CANDIDATE_CLAIM_NODE_TYPE: &str = "CandidateClaim";

/// The `type` property of the record of the claims one chunk grounds.
pub const CHUNK_FACTS_NODE_TYPE: &str = "ChunkFacts";

/// The edge from a claim to the chunk it was extracted from.
pub const EXTRACTED_FROM: &str = "extractedFrom";

/// The edge from a claim to the Document it derives from.
pub const DERIVED_FROM: &str = "derivedFrom";

/// The digest domain of a chunk facts record identity.
const CHUNK_FACTS_DOMAIN: &[u8] = b"eg/chunk-facts-identity/v1";

/// The `type` property of a stored entity a link joins.
pub const FACT_ENTITY_NODE_TYPE: &str = "FactEntity";

/// The digest domain of a fact atom identity.
const FACT_ATOM_DOMAIN: &[u8] = b"eg/fact-atom-identity/v1";

/// The digest domain of a claim identity: the fact without its evidence span.
const FACT_CLAIM_DOMAIN: &[u8] = b"eg/fact-claim-identity/v1";

/// The `type` property of the stored atoms a pair of entities shares.
pub const FACT_PAIR_NODE_TYPE: &str = "FactPair";

/// The digest domain of an entity pair identity.
const FACT_PAIR_DOMAIN: &[u8] = b"eg/fact-pair-identity/v1";

/// The digest domain of a linked entity identity.
const FACT_ENTITY_DOMAIN: &[u8] = b"eg/fact-entity-identity/v1";

/// The digest domain of a fact unit hash (MEDULLA-INGEST-WORKER-R701.5).
const FACT_UNIT_DOMAIN: &[u8] = b"eg/extracted-fact";

const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn invalid(detail: impl std::fmt::Display) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::SourceBatchKindMismatch.as_str()
    )
}

/// The writes of one admitted `facts` batch.
#[derive(Debug, Default)]
pub struct FactAdmission {
    /// One claim node, one atom node and one `atom_of` edge per kept fact.
    pub methods: Vec<Method>,
    /// The nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// Facts stored.
    pub admitted: u64,
    /// Repeated triples dropped.
    pub duplicates: u64,
    /// Stored facts that grounded to an ontology class.
    pub grounded: u64,
    /// The per-unit outcomes and the links created (MEDULLA-INGEST-WORKER-R701.5, R701.7).
    pub report: FactReport,
}

/// A claim the graph already holds, read before admission.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldClaim {
    /// The unit hash stored with the claim.
    pub unit_hash: String,
    /// The stored claim's confidence.
    pub confidence: f64,
    /// The chunks the claim was extracted from.
    pub chunks: BTreeSet<String>,
    /// The Documents the claim derives from.
    pub documents: BTreeSet<String>,
    /// The Document each grounding chunk belongs to, so the Documents of the surviving
    /// chunks can be recomputed when a chunk is dropped.
    pub chunk_documents: BTreeMap<String, String>,
    /// The stored node properties, rewritten whole when only the chunk set changes.
    pub properties: serde_json::Value,
}

/// The chunk a `facts` batch names, read before admission
/// (MEDULLA-UNIFIED-DATA-PLANE-R651.7.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldChunk {
    /// The chunk node id the batch names.
    pub chunk_id: String,
    /// The Document node the chunk belongs to.
    pub document_id: String,
    /// The hex SHA-256 of the chunk's stored text.
    pub content_hash: String,
    /// The claim ids an earlier extraction of this chunk grounded.
    pub prior_claims: BTreeSet<String>,
}

/// The hex SHA-256 of a chunk's stored text: the chunk digest.
pub fn chunk_content_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The scope key every `ChunkFacts` record is stored under: the opaque tenant scope of the
/// verified request. The facts batch handler and the enrichment handler both derive it here.
pub fn chunk_facts_scope(
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
) -> String {
    medulla_server_api::carrier::tenant_scope_of(verified.tenant())
}

/// The node id of the record of the claims one chunk grounds.
pub fn chunk_facts_id(tenant: &str, chunk_id: &str) -> Result<String, String> {
    let digest = Digest256::framed(
        CHUNK_FACTS_DOMAIN,
        &[tenant.as_bytes(), chunk_id.as_bytes()],
    )?;
    Ok(format!("chunk_facts:{}", digest.to_hex()))
}

/// The claim ids a stored chunk facts record holds.
pub fn held_chunk_claims(properties: &serde_json::Value) -> BTreeSet<String> {
    string_set(properties.get("claim_ids"))
}

/// A chunk node's properties as the chunk a batch names.
pub fn held_chunk(
    chunk_id: &str,
    properties: &serde_json::Value,
    prior_claims: BTreeSet<String>,
) -> Result<HeldChunk, String> {
    let document_id = properties
        .get("document_id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty());
    let text = properties.get("text").and_then(|value| value.as_str());
    match (document_id, text) {
        (Some(document_id), Some(text)) => Ok(HeldChunk {
            chunk_id: chunk_id.to_string(),
            document_id: document_id.to_string(),
            content_hash: chunk_content_hash(text),
            prior_claims,
        }),
        _ => Err(invalid(
            "source_chunk names a node that is not a stored chunk",
        )),
    }
}

fn string_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    value
        .and_then(|value| value.as_object())
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, item)| item.as_str().map(|item| (key.clone(), item.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn string_set(value: Option<&serde_json::Value>) -> BTreeSet<String> {
    value
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The ordered entity pair a fact's subject and object form; none when they are equal.
fn entity_pair(fact: &ExtractedFact) -> Option<(String, String)> {
    let (a, b) = (fact.subject.as_str(), fact.object.as_str());
    match a.cmp(b) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some((a.to_string(), b.to_string())),
        std::cmp::Ordering::Greater => Some((b.to_string(), a.to_string())),
    }
}

/// The node id of the stored atoms an ordered entity pair shares.
pub fn pair_node_id(tenant: &str, first: &str, second: &str) -> Result<String, String> {
    let digest = Digest256::framed(
        FACT_PAIR_DOMAIN,
        &[tenant.as_bytes(), first.as_bytes(), second.as_bytes()],
    )?;
    Ok(format!("fact_pair:{}", digest.to_hex()))
}

/// The pair node ids a decoded batch may extend, for the held-pairs read.
pub fn held_pair_ids(tenant: &str, decoded: &DecodedFacts) -> Result<BTreeSet<String>, String> {
    decoded
        .batch
        .facts
        .iter()
        .filter_map(entity_pair)
        .map(|(first, second)| pair_node_id(tenant, &first, &second))
        .collect()
}

/// The atom ids a stored pair node holds.
pub fn held_pair_atoms(properties: &serde_json::Value) -> Option<BTreeSet<String>> {
    properties
        .get("atoms")?
        .as_array()?
        .iter()
        .map(|atom| atom.as_str().map(str::to_string))
        .collect()
}

/// The decoded elements of a `facts` extension.
#[derive(Debug)]
pub struct DecodedFacts {
    /// The facts that carry a valid evidence span.
    pub batch: FactBatch,
    /// The refusal text of each fact without one (MEDULLA-INGEST-WORKER-R701.4).
    pub rejected: Vec<String>,
}

/// The stored claim properties a held claim is read from.
pub fn held_claim(properties: &serde_json::Value) -> Option<HeldClaim> {
    Some(HeldClaim {
        unit_hash: properties.get("unit_hash")?.as_str()?.to_string(),
        confidence: properties.get("fact")?.get("confidence")?.as_f64()?,
        chunks: string_set(properties.get("chunks")),
        documents: string_set(properties.get("documents")),
        chunk_documents: string_map(properties.get("chunk_documents")),
        properties: properties.clone(),
    })
}

/// The claim node ids a decoded batch may update, and the claims an earlier extraction
/// of its chunk grounded, for the held-claims read.
pub fn held_claim_ids(
    tenant: &str,
    decoded: &DecodedFacts,
    chunk: Option<&HeldChunk>,
) -> Result<BTreeSet<String>, String> {
    let mut ids: BTreeSet<String> = decoded
        .batch
        .facts
        .iter()
        .map(|fact| claim_node_id(tenant, fact))
        .collect::<Result<_, _>>()?;
    if let Some(chunk) = chunk {
        ids.extend(chunk.prior_claims.iter().cloned());
    }
    Ok(ids)
}

/// Decode the elements of a `facts` extension.
pub fn decode_facts(items: &[SourceJson]) -> Result<DecodedFacts, String> {
    if items.len() > MAX_FACTS {
        return Err(invalid(format!(
            "a facts batch holds at most {MAX_FACTS} facts"
        )));
    }
    let mut rejected = Vec::new();
    let mut facts = Vec::new();
    for item in items {
        let value = item.value()?;
        // A fact without a valid evidence span is rejected on its own, never the batch.
        let evidence = match value.get("evidence") {
            Some(evidence) => serde_json::from_value::<EvidenceSpan>(evidence.clone())
                .err()
                .map(|error| error.to_string()),
            None => Some("fact has no evidence span".to_string()),
        };
        if let Some(detail) = evidence {
            rejected.push(detail);
            continue;
        }
        facts.push(decode_fact(value)?);
    }
    Ok(DecodedFacts {
        batch: FactBatch { facts },
        rejected,
    })
}

fn decode_fact(value: serde_json::Value) -> Result<ExtractedFact, String> {
    let fact: ExtractedFact =
        serde_json::from_value(value).map_err(|error| invalid(format!("fact: {error}")))?;
    if !fact.confidence.is_finite() || !(0.0..=1.0).contains(&fact.confidence) {
        return Err(invalid("fact confidence must be a number in [0, 1]"));
    }
    if fact.subject.trim().is_empty()
        || fact.predicate.trim().is_empty()
        || fact.object.trim().is_empty()
    {
        return Err(invalid(
            "fact subject, predicate and object must not be blank",
        ));
    }
    Ok(fact)
}

fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

type Cached = Option<(Digest256, Arc<GroundingCache>)>;

/// The grounding cache of the graph's compiled ontology, computed once per
/// schema identity: each property with a declared domain and range maps its
/// local name to those classes, and each named individual maps its label, or
/// its local name, to its class. `reasoner_closure` is the `rdfs:subClassOf`
/// closure the graph's reasoner already holds (MEDULLA-INGEST-WORKER-R701.9);
/// when it is given the cache takes it as `subclass_of` and the closure of the
/// ontology's own edges is not used. Without it the cache keeps the closure it
/// built from the ontology, once per schema identity.
pub fn grounding_cache(
    sources: &GraphSchemaSources,
    reasoner_closure: Option<&BTreeMap<String, BTreeSet<String>>>,
) -> Result<Arc<GroundingCache>, String> {
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let digest = sources.composed_digest();
    let base = {
        let mut cache = CACHE.lock().map_err(|_| "grounding cache poisoned")?;
        match cache.as_ref() {
            Some((cached, built)) if *cached == digest => Arc::clone(built),
            _ => {
                let documents: Vec<&str> = sources.ontologies().map(|(_, text)| text).collect();
                let built = Arc::new(build_grounding_cache(&documents)?);
                *cache = Some((digest, Arc::clone(&built)));
                built
            }
        }
    };
    Ok(match reasoner_closure {
        Some(closure) => with_reasoner_closure(&base, closure),
        None => base,
    })
}

/// `base` with its `subclass_of` replaced by the closure the reasoner holds.
pub fn with_reasoner_closure(
    base: &GroundingCache,
    closure: &BTreeMap<String, BTreeSet<String>>,
) -> Arc<GroundingCache> {
    Arc::new(GroundingCache {
        subclass_of: closure.clone(),
        ..base.clone()
    })
}

/// Build a grounding cache from Turtle ontology documents.
pub fn build_grounding_cache(ontologies: &[&str]) -> Result<GroundingCache, String> {
    use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};
    let mut domains: BTreeMap<String, String> = BTreeMap::new();
    let mut ranges: BTreeMap<String, String> = BTreeMap::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let mut types: BTreeMap<String, String> = BTreeMap::new();
    let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for text in ontologies {
        for triple in medulla_rdf::mapping::parse_turtle(text)? {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            let subject = subject.as_str().to_string();
            match (triple.predicate.as_str(), &triple.object) {
                (RDFS_DOMAIN, Term::NamedNode(class)) => {
                    domains.insert(subject, class.as_str().to_string());
                }
                (RDFS_RANGE, Term::NamedNode(class)) => {
                    ranges.insert(subject, class.as_str().to_string());
                }
                (RDFS_LABEL, Term::Literal(label)) => {
                    labels.insert(subject, label.value().to_string());
                }
                (RDF_TYPE, Term::NamedNode(class)) => {
                    types.insert(subject, class.as_str().to_string());
                }
                (RDFS_SUBCLASS_OF, Term::NamedNode(parent)) => {
                    direct
                        .entry(subject)
                        .or_default()
                        .insert(parent.as_str().to_string());
                }
                _ => {}
            }
        }
    }
    let mut cache = GroundingCache {
        subclass_of: subclass_closure(&direct).map_err(|refusal| refusal.to_string())?,
        ..GroundingCache::default()
    };
    for (property, domain) in &domains {
        if let Some(range) = ranges.get(property) {
            cache.predicates.insert(
                local_name(property).to_string(),
                (domain.clone(), range.clone()),
            );
        }
    }
    for (individual, class) in &types {
        // Only a named individual of a declared class is an entity; a class or
        // a property typed by the vocabulary itself is not.
        if domains.contains_key(individual) || ranges.contains_key(individual) {
            continue;
        }
        let name = labels
            .get(individual)
            .cloned()
            .unwrap_or_else(|| local_name(individual).to_string());
        cache.classes.insert(name, class.clone());
    }
    Ok(cache)
}

/// The evidence span reference of a fact: the source, then the half-open span.
pub fn span_reference(fact: &ExtractedFact) -> String {
    format!(
        "{}#{}-{}",
        fact.evidence.source_ref, fact.evidence.start, fact.evidence.end
    )
}

/// The digest of a fact's atom identity: tenant, triple and evidence span.
fn fact_digest(tenant: &str, fact: &ExtractedFact) -> Result<String, String> {
    let span = span_reference(fact);
    Ok(Digest256::framed(
        FACT_ATOM_DOMAIN,
        &[
            tenant.as_bytes(),
            fact.subject.as_bytes(),
            fact.predicate.as_bytes(),
            fact.object.as_bytes(),
            span.as_bytes(),
        ],
    )?
    .to_hex())
}

/// The node id of the candidate claim stored for a fact: the digest of tenant
/// and triple, without the evidence span (MEDULLA-INGEST-WORKER-R701.5).
pub fn claim_node_id(tenant: &str, fact: &ExtractedFact) -> Result<String, String> {
    let digest = Digest256::framed(
        FACT_CLAIM_DOMAIN,
        &[
            tenant.as_bytes(),
            fact.subject.as_bytes(),
            fact.predicate.as_bytes(),
            fact.object.as_bytes(),
        ],
    )?;
    Ok(format!("candidate_claim:{}", digest.to_hex()))
}

/// The unit hash of a fact: SHA-256 framed under `eg/extracted-fact` over
/// subject, predicate, object and evidence span (MEDULLA-INGEST-WORKER-R701.5).
pub fn unit_hash(fact: &ExtractedFact) -> Result<String, String> {
    let span = span_reference(fact);
    Ok(Digest256::framed(
        FACT_UNIT_DOMAIN,
        &[
            fact.subject.as_bytes(),
            fact.predicate.as_bytes(),
            fact.object.as_bytes(),
            span.as_bytes(),
        ],
    )?
    .to_hex())
}

fn entity_node_id(tenant: &str, name: &str) -> Result<String, String> {
    let digest = Digest256::framed(FACT_ENTITY_DOMAIN, &[tenant.as_bytes(), name.as_bytes()])?;
    Ok(format!("fact_entity:{}", digest.to_hex()))
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| invalid(format!("fact encoding failed: {error}")))
}

/// Add the chunk and Document of a claim to its stored properties.
fn ground_claim_properties(
    properties: &mut serde_json::Value,
    before: &HeldClaim,
    chunk: &HeldChunk,
) {
    let mut chunks = before.chunks.clone();
    chunks.insert(chunk.chunk_id.clone());
    let mut documents = before.documents.clone();
    documents.insert(chunk.document_id.clone());
    properties["chunks"] = serde_json::json!(chunks);
    let mut chunk_documents = before.chunk_documents.clone();
    chunk_documents.insert(chunk.chunk_id.clone(), chunk.document_id.clone());
    properties["documents"] = serde_json::json!(documents);
    properties["chunk_documents"] = serde_json::json!(chunk_documents);
}

/// The `extractedFrom` edge to the chunk and, once per Document, the `derivedFrom` edge to it.
fn push_grounding_edges(
    methods: &mut Vec<Method>,
    claim_id: &str,
    before: &HeldClaim,
    chunk: &HeldChunk,
) -> Result<(), String> {
    methods.push(Method::AddEdge {
        source_id: claim_id.to_string(),
        target_id: chunk.chunk_id.clone(),
        properties_msgpack: encode(&serde_json::json!({ "relationship": EXTRACTED_FROM }))?,
    });
    if !before.documents.contains(&chunk.document_id) {
        methods.push(Method::AddEdge {
            source_id: claim_id.to_string(),
            target_id: chunk.document_id.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": DERIVED_FROM }))?,
        });
    }
    Ok(())
}

/// Apply the supersession rule to one stored claim whose grounding falls to `remaining`
/// chunks: a claim read from no remaining chunk is removed with its atom; otherwise it is
/// rewritten with the remaining chunk set and loses its `extractedFrom` edge to each of
/// `dropped_edges`.
fn supersede_claim(
    tenant: &str,
    claim_id: &str,
    stored: &HeldClaim,
    remaining: BTreeSet<String>,
    dropped_edges: &[String],
    admission: &mut FactAdmission,
) -> Result<(), String> {
    if remaining.is_empty() {
        admission.methods.push(Method::RemoveNode {
            node_id: claim_id.to_string(),
        });
        if let Some(fact) = stored
            .properties
            .get("fact")
            .and_then(|fact| serde_json::from_value::<ExtractedFact>(fact.clone()).ok())
        {
            admission.methods.push(Method::RemoveNode {
                node_id: atom_node_id(&fact_digest(tenant, &fact)?),
            });
        }
        return Ok(());
    }
    // The Documents and `derivedFrom` edges follow the surviving chunks: a Document no
    // surviving chunk belongs to stops grounding the claim.
    let chunk_documents: BTreeMap<String, String> = stored
        .chunk_documents
        .iter()
        .filter(|(chunk, _)| remaining.contains(*chunk))
        .map(|(chunk, document)| (chunk.clone(), document.clone()))
        .collect();
    let documents: BTreeSet<String> = chunk_documents.values().cloned().collect();
    for document in stored.documents.difference(&documents) {
        admission.methods.push(Method::RemoveEdge {
            source_id: claim_id.to_string(),
            target_id: document.clone(),
            relationship: None,
        });
    }
    let mut properties = stored.properties.clone();
    properties["chunks"] = serde_json::json!(remaining);
    properties["documents"] = serde_json::json!(documents);
    properties["chunk_documents"] = serde_json::json!(chunk_documents);
    for target in dropped_edges {
        admission.methods.push(Method::RemoveEdge {
            source_id: claim_id.to_string(),
            target_id: target.clone(),
            relationship: None,
        });
    }
    admission.methods.push(Method::AddNode {
        node_id: claim_id.to_string(),
        properties_msgpack: encode(&properties)?,
    });
    admission.node_ids.push(claim_id.to_string());
    Ok(())
}

/// The writes that supersede the claims grounded in chunks an enrichment removes (a new
/// Document digest, MEDULLA-UNIFIED-DATA-PLANE-R651.7.7): for each removed chunk the
/// `chunk_facts` record is read through `read` (node id to decoded properties) and removed,
/// and every claim it lists follows the rule of a re-extraction that states nothing: a claim
/// read from no surviving chunk is removed with its atom, any other keeps only its surviving
/// chunks. The removed chunk nodes carry their `extractedFrom` edges away, so no edge to a
/// removed chunk is left.
pub fn supersede_removed_chunks(
    tenant: &str,
    chunk_ids: &[String],
    read: &dyn Fn(&str) -> Option<serde_json::Value>,
) -> Result<Vec<Method>, String> {
    let removed: BTreeSet<&String> = chunk_ids.iter().collect();
    let mut admission = FactAdmission::default();
    let mut claims: BTreeMap<String, HeldClaim> = BTreeMap::new();
    for chunk_id in chunk_ids {
        let record_id = chunk_facts_id(tenant, chunk_id)?;
        let Some(record) = read(&record_id) else {
            continue;
        };
        for claim_id in held_chunk_claims(&record) {
            if claims.contains_key(&claim_id) {
                continue;
            }
            if let Some(claim) = read(&claim_id).as_ref().and_then(held_claim) {
                claims.insert(claim_id, claim);
            }
        }
        admission
            .methods
            .push(Method::RemoveNode { node_id: record_id });
    }
    for (claim_id, stored) in &claims {
        let remaining: BTreeSet<String> = stored
            .chunks
            .iter()
            .filter(|chunk| !removed.contains(chunk))
            .cloned()
            .collect();
        supersede_claim(tenant, claim_id, stored, remaining, &[], &mut admission)?;
    }
    Ok(admission.methods)
}

/// Record the claims the chunk now grounds and supersede the claims an earlier extraction
/// of it grounded that this batch no longer states: a claim read from no other chunk is
/// removed with its atom, one another chunk also grounds loses only this chunk. The
/// outcome of the chunk joins the report.
fn supersede_chunk_claims(
    tenant: &str,
    chunk: &HeldChunk,
    held: &BTreeMap<String, HeldClaim>,
    restated: &BTreeSet<String>,
    admission: &mut FactAdmission,
) -> Result<(), String> {
    let mut superseded = 0;
    for prior in chunk.prior_claims.difference(restated) {
        let Some(stored) = held.get(prior) else {
            continue;
        };
        superseded += 1;
        let mut remaining = stored.chunks.clone();
        remaining.remove(&chunk.chunk_id);
        supersede_claim(
            tenant,
            prior,
            stored,
            remaining,
            std::slice::from_ref(&chunk.chunk_id),
            admission,
        )?;
    }
    let record_id = chunk_facts_id(tenant, &chunk.chunk_id)?;
    admission.methods.push(Method::AddNode {
        node_id: record_id.clone(),
        properties_msgpack: encode(&serde_json::json!({
            "type": CHUNK_FACTS_NODE_TYPE,
            "tenant": tenant,
            "chunk_id": chunk.chunk_id,
            "document_id": chunk.document_id,
            "content_hash": chunk.content_hash,
            "claim_ids": restated,
        }))?,
    });
    admission.node_ids.push(record_id);
    let report = &admission.report;
    let outcome = FactChunk {
        chunk_id: chunk.chunk_id.clone(),
        content_hash: chunk.content_hash.clone(),
        created: report.created,
        updated: report.updated,
        unchanged: report.unchanged,
        rejected: report.rejected,
        superseded,
    };
    admission.report.chunks.push(outcome);
    Ok(())
}

/// Ground each fact, drop repeated triples, classify the rest against the held
/// claims and compile the claim, atom and link writes, in batch order
/// (MEDULLA-INGEST-WORKER-R701.3, R701.5, R701.7). An unchanged claim writes
/// nothing; an updated claim keeps its node and combines confidence as
/// `1 - (1 - old) * (1 - new)`. Every pair of entities a new atom mentions is recorded
/// on a `fact_pair` node that holds its atom ids; entities that co-occur in at least
/// `min_shared` stored atoms, counted across batches through `held_pairs`, get a
/// `related_to` edge. With a `chunk`, every kept claim also gets an `extractedFrom` edge to
/// the chunk and a `derivedFrom` edge to its Document, and the claims an earlier
/// extraction of the chunk grounded but this batch no longer states are superseded
/// (MEDULLA-UNIFIED-DATA-PLANE-R651.7.7).
pub fn admit_facts(
    tenant: &str,
    decoded: &DecodedFacts,
    cache: &GroundingCache,
    held: &BTreeMap<String, HeldClaim>,
    held_pairs: &BTreeMap<String, BTreeSet<String>>,
    min_shared: u64,
    chunk: Option<&HeldChunk>,
) -> Result<FactAdmission, String> {
    let mut restated: BTreeSet<String> = BTreeSet::new();
    let mut deduper = FactDeduper::default();
    let mut admission = FactAdmission::default();
    let mut created: Vec<CandidateClaim> = Vec::new();
    let mut new_atoms: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for fact in &decoded.batch.facts {
        if !deduper.admit(fact) {
            admission.duplicates += 1;
            continue;
        }
        let claim_id = claim_node_id(tenant, fact)?;
        let hash = unit_hash(fact)?;
        let (outcome, confidence) = match held.get(&claim_id) {
            None => (FactOutcome::Created, fact.confidence),
            Some(stored) if stored.unit_hash == hash => (FactOutcome::Unchanged, stored.confidence),
            Some(stored) => (
                FactOutcome::Updated,
                1.0 - (1.0 - stored.confidence) * (1.0 - fact.confidence),
            ),
        };
        admission.report.record(FactUnit {
            claim_id: claim_id.clone(),
            unit_hash: hash.clone(),
            outcome,
            detail: String::new(),
        });
        restated.insert(claim_id.clone());
        let stored_claim = held.get(&claim_id);
        let new_to_chunk = chunk.is_some_and(|chunk| {
            stored_claim.is_none_or(|stored| !stored.chunks.contains(&chunk.chunk_id))
        });
        if outcome == FactOutcome::Unchanged {
            if let (Some(chunk), Some(stored), true) = (chunk, stored_claim, new_to_chunk) {
                // The same claim read from another chunk: only its grounding changes.
                let mut properties = stored.properties.clone();
                ground_claim_properties(&mut properties, stored, chunk);
                admission.methods.push(Method::AddNode {
                    node_id: claim_id.clone(),
                    properties_msgpack: encode(&properties)?,
                });
                push_grounding_edges(&mut admission.methods, &claim_id, stored, chunk)?;
                admission.node_ids.push(claim_id);
            }
            continue;
        }
        let mut claim: CandidateClaim = ground_fact(fact.clone(), cache);
        claim.fact.confidence = confidence;
        if matches!(claim.grounding, GroundingResult::Grounded { .. }) {
            admission.grounded += 1;
        }
        let digest = fact_digest(tenant, fact)?;
        let atom = KnowledgeAtom {
            atom_id: digest,
            kind: AtomKind::Fact,
            text: format!("{} {} {}", fact.subject, fact.predicate, fact.object),
            provenance_key: span_reference(fact),
            tenant: tenant.to_string(),
        };
        check_atom_provenance(&atom).map_err(|refusal| invalid(refusal.code()))?;
        let atom_id = atom_node_id(&atom.atom_id);
        let mut claim_properties = serde_json::json!({
            "type": CANDIDATE_CLAIM_NODE_TYPE,
            "tenant": tenant,
            "fact": claim.fact,
            "grounding": claim.grounding,
            "unit_hash": hash,
        });
        let empty = HeldClaim {
            unit_hash: String::new(),
            confidence: 0.0,
            chunks: BTreeSet::new(),
            documents: BTreeSet::new(),
            chunk_documents: BTreeMap::new(),
            properties: serde_json::Value::Null,
        };
        let before = stored_claim.unwrap_or(&empty);
        if let Some(chunk) = chunk {
            ground_claim_properties(&mut claim_properties, before, chunk);
        }
        admission.methods.push(Method::AddNode {
            node_id: claim_id.clone(),
            properties_msgpack: encode(&claim_properties)?,
        });
        admission.methods.push(Method::AddNode {
            node_id: atom_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": ATOM_NODE_TYPE,
                "atom_id": atom.atom_id,
                "kind": atom.kind,
                "text": atom.text,
                "provenance_key": atom.provenance_key,
                "tenant": atom.tenant,
            }))?,
        });
        admission.methods.push(Method::AddEdge {
            source_id: atom_id.clone(),
            target_id: claim_id.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": ATOM_OF }))?,
        });
        if let (Some(chunk), true) = (chunk, new_to_chunk) {
            push_grounding_edges(&mut admission.methods, &claim_id, before, chunk)?;
        }
        admission.node_ids.push(claim_id);
        admission.node_ids.push(atom_id.clone());
        admission.admitted += 1;
        if outcome == FactOutcome::Created {
            if let Some(pair) = entity_pair(fact) {
                new_atoms.entry(pair).or_default().push(atom_id.clone());
            }
            created.push(claim);
        }
    }
    for detail in &decoded.rejected {
        admission.report.record(FactUnit {
            claim_id: String::new(),
            unit_hash: String::new(),
            outcome: FactOutcome::Rejected,
            detail: detail.clone(),
        });
    }
    if let Some(chunk) = chunk {
        supersede_chunk_claims(tenant, chunk, held, &restated, &mut admission)?;
    }
    let mut stored: BTreeMap<(String, String), u64> = BTreeMap::new();
    for (first, second) in new_atoms.keys() {
        if let Some(atoms) = held_pairs.get(&pair_node_id(tenant, first, second)?) {
            stored.insert((first.clone(), second.clone()), atoms.len() as u64);
        }
    }
    for ((first, second), added) in &new_atoms {
        let id = pair_node_id(tenant, first, second)?;
        let mut atoms: BTreeSet<String> = held_pairs.get(&id).cloned().unwrap_or_default();
        atoms.extend(added.iter().cloned());
        admission.methods.push(Method::AddNode {
            node_id: id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": FACT_PAIR_NODE_TYPE,
                "first": first,
                "second": second,
                "tenant": tenant,
                "atoms": atoms,
            }))?,
        });
        admission.node_ids.push(id);
    }
    let mut entities: BTreeSet<String> = BTreeSet::new();
    for link in infer_links(&created, &stored, min_shared) {
        let ids = [
            entity_node_id(tenant, &link.first)?,
            entity_node_id(tenant, &link.second)?,
        ];
        for (id, name) in ids.iter().zip([&link.first, &link.second]) {
            if entities.insert(id.clone()) {
                admission.methods.push(Method::AddNode {
                    node_id: id.clone(),
                    properties_msgpack: encode(&serde_json::json!({
                        "type": FACT_ENTITY_NODE_TYPE,
                        "name": name,
                        "tenant": tenant,
                    }))?,
                });
                admission.node_ids.push(id.clone());
            }
        }
        let [source_id, target_id] = ids;
        admission.methods.push(Method::AddEdge {
            source_id,
            target_id,
            properties_msgpack: encode(&serde_json::json!({
                "relationship": RELATED_TO,
                "shared_mentions": link.shared_mentions,
            }))?,
        });
        if link.previously_shared < min_shared {
            admission.report.links += 1;
        }
    }
    Ok(admission)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONTOLOGY: &str = r#"
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
        @prefix ex: <http://example.org/o#> .
        ex:wrote rdfs:domain ex:Person ; rdfs:range ex:Work .
        ex:Ada a ex:Person ; rdfs:label "Ada Lovelace" .
        ex:NoteG a ex:Work ; rdfs:label "Note G" .
    "#;

    fn fact(subject: &str, object: &str, start: u64) -> ExtractedFact {
        serde_json::from_value(serde_json::json!({
            "subject": subject,
            "predicate": "wrote",
            "object": object,
            "evidence": {"source_ref": "doc:ada", "start": start, "end": start + 20},
            "confidence": 0.9
        }))
        .unwrap()
    }

    fn node_properties(method: &Method) -> serde_json::Value {
        match method {
            Method::AddNode {
                properties_msgpack, ..
            } => rmp_serde::from_slice(properties_msgpack).unwrap(),
            other => panic!("expected AddNode, got {other:?}"),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R701.3
    #[test]
    fn facts_reach_the_atom_store() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let batch = DecodedFacts {
            rejected: Vec::new(),
            batch: FactBatch {
                facts: vec![
                    // Subject and object swapped: direction repair grounds it.
                    fact("Note G", "Ada Lovelace", 10),
                    // The same triple again drops; an unknown entity stays a claim.
                    fact("Note G", "Ada Lovelace", 10),
                    fact("Somebody", "Something", 50),
                ],
            },
        };
        let admission = admit_facts(
            "t1",
            &batch,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        assert_eq!(admission.admitted, 2);
        assert_eq!(admission.duplicates, 1);
        assert_eq!(admission.grounded, 1);
        // Two claims, two atoms, two edges and one pair node per entity pair.
        assert_eq!(admission.methods.len(), 8);
        assert_eq!(admission.node_ids.len(), 6);

        let claim = node_properties(&admission.methods[0]);
        assert_eq!(claim["type"], CANDIDATE_CLAIM_NODE_TYPE);
        assert_eq!(claim["grounding"]["status"], "grounded");
        assert_eq!(claim["grounding"]["direction_repaired"], true);
        let atom = node_properties(&admission.methods[1]);
        assert_eq!(atom["type"], ATOM_NODE_TYPE);
        assert_eq!(atom["kind"], "fact");
        assert_eq!(atom["provenance_key"], "doc:ada#10-30");
        assert_eq!(atom["tenant"], "t1");
        let ungrounded = node_properties(&admission.methods[3]);
        assert_eq!(ungrounded["grounding"]["status"], "ungrounded");
        match &admission.methods[2] {
            Method::AddEdge {
                source_id,
                target_id,
                ..
            } => {
                assert_eq!(source_id, &admission.node_ids[1]);
                assert_eq!(target_id, &admission.node_ids[0]);
            }
            other => panic!("expected AddEdge, got {other:?}"),
        }
    }

    fn decoded(facts: Vec<ExtractedFact>) -> DecodedFacts {
        DecodedFacts {
            batch: FactBatch { facts },
            rejected: Vec::new(),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R701.5
    #[test]
    fn a_held_claim_classifies_each_unit() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let first = fact("Ada Lovelace", "Note G", 10);
        let batch = decoded(vec![first.clone()]);
        let created = admit_facts(
            "t1",
            &batch,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        assert_eq!(created.report.created, 1);
        let unit = &created.report.entries[0];
        assert_eq!(unit.outcome, FactOutcome::Created);
        let claim = node_properties(&created.methods[0]);
        let stored = held_claim(&claim).expect("held claim");
        assert_eq!(stored.unit_hash, unit.unit_hash);

        let mut held = BTreeMap::new();
        held.insert(unit.claim_id.clone(), stored);
        let again = admit_facts("t1", &batch, &cache, &held, &BTreeMap::new(), 2, None).unwrap();
        assert_eq!(again.report.unchanged, 1);
        assert_eq!(again.report.entries[0].unit_hash, unit.unit_hash);
        assert!(again.methods.is_empty());

        // The same triple at another span keeps the claim id and updates it.
        let moved = decoded(vec![fact("Ada Lovelace", "Note G", 90)]);
        let updated = admit_facts("t1", &moved, &cache, &held, &BTreeMap::new(), 2, None).unwrap();
        assert_eq!(updated.report.updated, 1);
        assert_eq!(updated.report.entries[0].claim_id, unit.claim_id);
        assert_ne!(updated.report.entries[0].unit_hash, unit.unit_hash);
        let merged = node_properties(&updated.methods[0]);
        let confidence = merged["fact"]["confidence"].as_f64().unwrap();
        assert!((confidence - (1.0 - 0.1 * 0.1)).abs() < 1e-9);
    }

    // spec: MEDULLA-INGEST-WORKER-R701.5
    #[test]
    fn a_fact_without_an_evidence_span_is_rejected() {
        let item = SourceJson::new(serde_json::json!({
            "subject": "a", "predicate": "p", "object": "b", "confidence": 0.5
        }))
        .unwrap();
        let decoded = decode_facts(&[item]).unwrap();
        let cache = GroundingCache::default();
        let admission = admit_facts(
            "t1",
            &decoded,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        assert_eq!(admission.report.rejected, 1);
        assert_eq!(admission.report.entries[0].outcome, FactOutcome::Rejected);
        assert!(admission.methods.is_empty());
    }

    // spec: MEDULLA-INGEST-WORKER-R701.7
    #[test]
    fn a_link_needs_enough_shared_mentions() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let mut second = fact("Ada Lovelace", "Note G", 40);
        second.predicate = "annotated".into();
        let two = decoded(vec![fact("Ada Lovelace", "Note G", 10), second]);
        let linked = admit_facts(
            "t1",
            &two,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        assert_eq!(linked.report.links, 1);
        let edges = linked
            .methods
            .iter()
            .filter(|method| matches!(method, Method::AddEdge { .. }))
            .count();
        assert_eq!(edges, 2 + 1);

        let one = decoded(vec![fact("Somebody", "Something", 10)]);
        let none = admit_facts(
            "t1",
            &one,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        assert_eq!(none.report.links, 0);
    }

    // spec: MEDULLA-INGEST-WORKER-R701.7
    #[test]
    fn a_link_forms_only_on_the_second_batch() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let first = admit_facts(
            "t1",
            &decoded(vec![fact("Ada Lovelace", "Note G", 10)]),
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            None,
        )
        .unwrap();
        // One shared atom: no link, but the pair node keeps the evidence.
        assert_eq!(first.report.links, 0);
        let pair = first
            .methods
            .iter()
            .map(node_properties_opt)
            .flatten()
            .find(|properties| properties["type"] == FACT_PAIR_NODE_TYPE)
            .expect("pair node");
        let mut held_pairs = BTreeMap::new();
        held_pairs.insert(
            pair_node_id("t1", "Ada Lovelace", "Note G").unwrap(),
            held_pair_atoms(&pair).expect("atoms"),
        );

        let mut other = fact("Ada Lovelace", "Note G", 40);
        other.predicate = "annotated".into();
        let second = admit_facts(
            "t1",
            &decoded(vec![other]),
            &cache,
            &BTreeMap::new(),
            &held_pairs,
            2,
            None,
        )
        .unwrap();
        assert_eq!(second.report.links, 1);
        let edge = second
            .methods
            .iter()
            .find_map(|method| match method {
                Method::AddEdge {
                    properties_msgpack, ..
                } => rmp_serde::from_slice::<serde_json::Value>(properties_msgpack).ok(),
                _ => None,
            })
            .filter(|properties| properties["relationship"] == RELATED_TO);
        assert_eq!(edge.expect("related_to edge")["shared_mentions"], 2);
    }

    fn node_properties_opt(method: &Method) -> Option<serde_json::Value> {
        match method {
            Method::AddNode {
                properties_msgpack, ..
            } => rmp_serde::from_slice(properties_msgpack).ok(),
            _ => None,
        }
    }

    fn chunk(prior: &[&str]) -> HeldChunk {
        HeldChunk {
            chunk_id: "chunk:doc:ada:0".into(),
            document_id: "doc:ada".into(),
            content_hash: chunk_content_hash("Ada wrote Note G."),
            prior_claims: prior.iter().map(|id| id.to_string()).collect(),
        }
    }

    fn edge_relations(admission: &FactAdmission) -> Vec<(String, String, String)> {
        admission
            .methods
            .iter()
            .filter_map(|method| match method {
                Method::AddEdge {
                    source_id,
                    target_id,
                    properties_msgpack,
                } => {
                    let properties: serde_json::Value =
                        rmp_serde::from_slice(properties_msgpack).ok()?;
                    Some((
                        source_id.clone(),
                        target_id.clone(),
                        properties["relationship"].as_str()?.to_string(),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.7
    #[test]
    fn a_chunk_batch_grounds_each_claim_in_the_same_commit() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let held_chunk = chunk(&[]);
        let batch = decoded(vec![fact("Ada Lovelace", "Note G", 10)]);
        let admission = admit_facts(
            "t1",
            &batch,
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            Some(&held_chunk),
        )
        .unwrap();
        let claim_id = admission.report.entries[0].claim_id.clone();
        let relations = edge_relations(&admission);
        assert!(relations.contains(&(
            claim_id.clone(),
            "chunk:doc:ada:0".into(),
            EXTRACTED_FROM.into()
        )));
        assert!(relations.contains(&(claim_id.clone(), "doc:ada".into(), DERIVED_FROM.into())));
        let claim = node_properties(&admission.methods[0]);
        assert_eq!(claim["chunks"], serde_json::json!(["chunk:doc:ada:0"]));
        assert_eq!(claim["documents"], serde_json::json!(["doc:ada"]));
        let record = admission
            .methods
            .iter()
            .filter_map(node_properties_opt)
            .find(|properties| properties["type"] == CHUNK_FACTS_NODE_TYPE)
            .expect("chunk facts record");
        assert_eq!(record["claim_ids"], serde_json::json!([claim_id]));
        let outcome = &admission.report.chunks[0];
        assert_eq!(outcome.chunk_id, "chunk:doc:ada:0");
        assert_eq!(
            outcome.content_hash,
            chunk_content_hash("Ada wrote Note G.")
        );
        assert_eq!((outcome.created, outcome.superseded), (1, 0));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.7
    #[test]
    fn a_re_extraction_supersedes_the_claims_it_no_longer_states() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let first_chunk = chunk(&[]);
        let first = admit_facts(
            "t1",
            &decoded(vec![
                fact("Ada Lovelace", "Note G", 10),
                fact("Somebody", "Something", 50),
            ]),
            &cache,
            &BTreeMap::new(),
            &BTreeMap::new(),
            2,
            Some(&first_chunk),
        )
        .unwrap();
        let mut held = BTreeMap::new();
        let mut prior = Vec::new();
        for method in &first.methods {
            if let Some(properties) = node_properties_opt(method) {
                if properties["type"] == CANDIDATE_CLAIM_NODE_TYPE {
                    let Method::AddNode { node_id, .. } = method else {
                        unreachable!()
                    };
                    held.insert(
                        node_id.clone(),
                        held_claim(&properties).expect("held claim"),
                    );
                    prior.push(node_id.as_str());
                }
            }
        }
        assert_eq!(prior.len(), 2);
        let second_chunk = chunk(&prior);
        let again = admit_facts(
            "t1",
            &decoded(vec![fact("Ada Lovelace", "Note G", 10)]),
            &cache,
            &held,
            &BTreeMap::new(),
            2,
            Some(&second_chunk),
        )
        .unwrap();
        assert_eq!(again.report.unchanged, 1);
        let outcome = &again.report.chunks[0];
        assert_eq!((outcome.unchanged, outcome.superseded), (1, 1));
        let removed = again
            .methods
            .iter()
            .filter(|method| matches!(method, Method::RemoveNode { .. }))
            .count();
        // The unstated claim and its atom.
        assert_eq!(removed, 2);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.7
    #[test]
    fn a_surviving_claim_recomputes_its_documents_and_derived_from_edges() {
        let cache = build_grounding_cache(&[ONTOLOGY]).unwrap();
        let fresh = |chunk_id: &str, document_id: &str, held: &BTreeMap<String, HeldClaim>| {
            let chunk = HeldChunk {
                chunk_id: chunk_id.into(),
                document_id: document_id.into(),
                content_hash: chunk_content_hash(chunk_id),
                prior_claims: BTreeSet::new(),
            };
            admit_facts(
                "t1",
                &decoded(vec![fact("Ada Lovelace", "Note G", 10)]),
                &cache,
                held,
                &BTreeMap::new(),
                2,
                Some(&chunk),
            )
            .unwrap()
        };
        let claim_of = |admission: &FactAdmission| {
            admission
                .methods
                .iter()
                .find_map(|method| match (method, node_properties_opt(method)) {
                    (Method::AddNode { node_id, .. }, Some(properties))
                        if properties["type"] == CANDIDATE_CLAIM_NODE_TYPE =>
                    {
                        Some((node_id.clone(), properties))
                    }
                    _ => None,
                })
                .expect("claim write")
        };
        let first = fresh("chunk:a", "doc:a", &BTreeMap::new());
        let (claim_id, properties) = claim_of(&first);
        let mut held = BTreeMap::new();
        held.insert(
            claim_id.clone(),
            held_claim(&properties).expect("held claim"),
        );
        let second = fresh("chunk:b", "doc:b", &held);
        let (_, properties) = claim_of(&second);
        assert_eq!(
            properties["documents"],
            serde_json::json!(["doc:a", "doc:b"])
        );
        let both = held_claim(&properties).expect("held claim");
        let mut stored = BTreeMap::new();
        stored.insert(claim_id.clone(), both);
        let record_id = chunk_facts_id("t1", "chunk:b").unwrap();
        let record = serde_json::json!({ "claim_ids": [claim_id.clone()] });
        let read = |id: &str| {
            if id == record_id {
                Some(record.clone())
            } else {
                stored.get(id).map(|claim| claim.properties.clone())
            }
        };
        let methods = supersede_removed_chunks("t1", &["chunk:b".to_string()], &read).unwrap();
        let rewritten = methods
            .iter()
            .find_map(|method| match (method, node_properties_opt(method)) {
                (Method::AddNode { node_id, .. }, Some(properties)) if *node_id == claim_id => {
                    Some(properties)
                }
                _ => None,
            })
            .expect("surviving claim rewritten");
        assert_eq!(rewritten["chunks"], serde_json::json!(["chunk:a"]));
        assert_eq!(rewritten["documents"], serde_json::json!(["doc:a"]));
        assert!(methods.iter().any(|method| matches!(
            method,
            Method::RemoveEdge { source_id, target_id, .. }
                if *source_id == claim_id && target_id == "doc:b"
        )));
        assert!(!methods.iter().any(|method| matches!(
            method,
            Method::RemoveEdge { target_id, .. } if target_id == "doc:a"
        )));
    }

    // spec: MEDULLA-INGEST-WORKER-R701.3
    #[test]
    fn decode_refuses_a_bad_confidence() {
        let item = SourceJson::new(serde_json::json!({
            "subject": "a", "predicate": "p", "object": "b",
            "evidence": {"source_ref": "doc", "start": 0, "end": 1},
            "confidence": 1.5
        }))
        .unwrap();
        assert!(decode_facts(&[item]).is_err());
    }
}
