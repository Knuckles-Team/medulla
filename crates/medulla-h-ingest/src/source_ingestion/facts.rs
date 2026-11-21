//! Admission of `facts` batches (MEDULLA-INGEST-WORKER-R701.3).
//!
//! The SDK sends only prose facts. The engine grounds each fact against the
//! compiled ontology of the graph (`ground_fact`), drops a repeated triple
//! (`FactDeduper`) and stores every kept fact twice: as a `candidate_claim`
//! node that carries the fact and its grounding, and as a `KnowledgeAtom` of
//! kind `Fact` whose provenance key is the evidence span reference. The atom
//! links to its claim by an `atom_of` edge.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::extracted_fact::{
    CandidateClaim, EvidenceSpan, ExtractedFact, FactBatch, FactOutcome, FactReport, FactUnit,
    GroundingResult,
};
use medulla_ingestion_types::ingest_rules::{
    check_atom_provenance, ground_fact, infer_links, FactDeduper, GroundingCache, RELATED_TO,
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

/// The `type` property of a stored entity a link joins.
pub const FACT_ENTITY_NODE_TYPE: &str = "FactEntity";

/// The digest domain of a fact atom identity.
const FACT_ATOM_DOMAIN: &[u8] = b"eg/fact-atom-identity/v1";

/// The digest domain of a claim identity: the fact without its evidence span.
const FACT_CLAIM_DOMAIN: &[u8] = b"eg/fact-claim-identity/v1";

/// The digest domain of a linked entity identity.
const FACT_ENTITY_DOMAIN: &[u8] = b"eg/fact-entity-identity/v1";

/// The digest domain of a fact unit hash (MEDULLA-INGEST-WORKER-R701.5).
const FACT_UNIT_DOMAIN: &[u8] = b"eg/extracted-fact";

const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
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
    })
}

/// The claim node ids a decoded batch may update, for the held-claims read.
pub fn held_claim_ids(tenant: &str, decoded: &DecodedFacts) -> Result<BTreeSet<String>, String> {
    decoded
        .batch
        .facts
        .iter()
        .map(|fact| claim_node_id(tenant, fact))
        .collect()
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
        return Err(invalid("fact subject, predicate and object must not be blank"));
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
/// its local name, to its class.
pub fn grounding_cache(sources: &GraphSchemaSources) -> Result<Arc<GroundingCache>, String> {
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let digest = sources.composed_digest();
    let mut cache = CACHE.lock().map_err(|_| "grounding cache poisoned")?;
    if let Some((cached, built)) = cache.as_ref() {
        if *cached == digest {
            return Ok(Arc::clone(built));
        }
    }
    let documents: Vec<&str> = sources.ontologies().map(|(_, text)| text).collect();
    let built = Arc::new(build_grounding_cache(&documents)?);
    *cache = Some((digest, Arc::clone(&built)));
    Ok(built)
}

/// Build a grounding cache from Turtle ontology documents.
pub fn build_grounding_cache(ontologies: &[&str]) -> Result<GroundingCache, String> {
    use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};
    let mut domains: BTreeMap<String, String> = BTreeMap::new();
    let mut ranges: BTreeMap<String, String> = BTreeMap::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let mut types: BTreeMap<String, String> = BTreeMap::new();
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
                _ => {}
            }
        }
    }
    let mut cache = GroundingCache::default();
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
    let digest = Digest256::framed(
        FACT_ENTITY_DOMAIN,
        &[tenant.as_bytes(), name.as_bytes()],
    )?;
    Ok(format!("fact_entity:{}", digest.to_hex()))
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| invalid(format!("fact encoding failed: {error}")))
}

/// Ground each fact, drop repeated triples, classify the rest against the held
/// claims and compile the claim, atom and link writes, in batch order
/// (MEDULLA-INGEST-WORKER-R701.3, R701.5, R701.7). An unchanged claim writes
/// nothing; an updated claim keeps its node and combines confidence as
/// `1 - (1 - old) * (1 - new)`. Entities that co-occur in at least `min_shared`
/// newly created atoms of the batch get a `related_to` edge.
pub fn admit_facts(
    tenant: &str,
    decoded: &DecodedFacts,
    cache: &GroundingCache,
    held: &BTreeMap<String, HeldClaim>,
    min_shared: u64,
) -> Result<FactAdmission, String> {
    let mut deduper = FactDeduper::default();
    let mut admission = FactAdmission::default();
    let mut created: Vec<CandidateClaim> = Vec::new();
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
        if outcome == FactOutcome::Unchanged {
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
        admission.methods.push(Method::AddNode {
            node_id: claim_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": CANDIDATE_CLAIM_NODE_TYPE,
                "tenant": tenant,
                "fact": claim.fact,
                "grounding": claim.grounding,
                "unit_hash": hash,
            }))?,
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
        admission.node_ids.push(claim_id);
        admission.node_ids.push(atom_id);
        admission.admitted += 1;
        if outcome == FactOutcome::Created {
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
    let mut entities: BTreeSet<String> = BTreeSet::new();
    for link in infer_links(&created, min_shared) {
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
        admission.report.links += 1;
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
        let admission = admit_facts("t1", &batch, &cache, &BTreeMap::new(), 2).unwrap();
        assert_eq!(admission.admitted, 2);
        assert_eq!(admission.duplicates, 1);
        assert_eq!(admission.grounded, 1);
        assert_eq!(admission.methods.len(), 6);
        assert_eq!(admission.node_ids.len(), 4);

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
            Method::AddEdge { source_id, target_id, .. } => {
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
        let created = admit_facts("t1", &batch, &cache, &BTreeMap::new(), 2).unwrap();
        assert_eq!(created.report.created, 1);
        let unit = &created.report.entries[0];
        assert_eq!(unit.outcome, FactOutcome::Created);
        let claim = node_properties(&created.methods[0]);
        let stored = held_claim(&claim).expect("held claim");
        assert_eq!(stored.unit_hash, unit.unit_hash);

        let mut held = BTreeMap::new();
        held.insert(unit.claim_id.clone(), stored);
        let again = admit_facts("t1", &batch, &cache, &held, 2).unwrap();
        assert_eq!(again.report.unchanged, 1);
        assert_eq!(again.report.entries[0].unit_hash, unit.unit_hash);
        assert!(again.methods.is_empty());

        // The same triple at another span keeps the claim id and updates it.
        let moved = decoded(vec![fact("Ada Lovelace", "Note G", 90)]);
        let updated = admit_facts("t1", &moved, &cache, &held, 2).unwrap();
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
        let admission = admit_facts("t1", &decoded, &cache, &BTreeMap::new(), 2).unwrap();
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
        let linked = admit_facts("t1", &two, &cache, &BTreeMap::new(), 2).unwrap();
        assert_eq!(linked.report.links, 1);
        let edges = linked
            .methods
            .iter()
            .filter(|method| matches!(method, Method::AddEdge { .. }))
            .count();
        assert_eq!(edges, 2 + 1);

        let one = decoded(vec![fact("Somebody", "Something", 10)]);
        let none = admit_facts("t1", &one, &cache, &BTreeMap::new(), 2).unwrap();
        assert_eq!(none.report.links, 0);
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
