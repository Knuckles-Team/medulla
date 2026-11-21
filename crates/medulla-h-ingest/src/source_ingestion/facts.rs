//! Admission of `facts` batches (MEDULLA-INGEST-WORKER-R701.3).
//!
//! The SDK sends only prose facts. The engine grounds each fact against the
//! compiled ontology of the graph (`ground_fact`), drops a repeated triple
//! (`FactDeduper`) and stores every kept fact twice: as a `candidate_claim`
//! node that carries the fact and its grounding, and as a `KnowledgeAtom` of
//! kind `Fact` whose provenance key is the evidence span reference. The atom
//! links to its claim by an `atom_of` edge.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::extracted_fact::{
    CandidateClaim, ExtractedFact, FactBatch, GroundingResult,
};
use medulla_ingestion_types::ingest_rules::{
    check_atom_provenance, ground_fact, FactDeduper, GroundingCache,
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

/// The digest domain of a fact atom identity.
const FACT_ATOM_DOMAIN: &[u8] = b"eg/fact-atom-identity/v1";

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
}

/// Decode the elements of a `facts` extension.
pub fn decode_facts(items: &[SourceJson]) -> Result<FactBatch, String> {
    if items.len() > MAX_FACTS {
        return Err(invalid(format!(
            "a facts batch holds at most {MAX_FACTS} facts"
        )));
    }
    let facts = items
        .iter()
        .map(|item| {
            let fact: ExtractedFact = serde_json::from_value(item.value()?)
                .map_err(|error| invalid(format!("fact: {error}")))?;
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
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(FactBatch { facts })
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

/// The node id of the candidate claim stored for a fact.
pub fn claim_node_id(tenant: &str, fact: &ExtractedFact) -> Result<String, String> {
    Ok(format!("candidate_claim:{}", fact_digest(tenant, fact)?))
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| invalid(format!("fact encoding failed: {error}")))
}

/// Ground each fact, drop repeated triples and compile the claim and atom
/// writes of the rest, in batch order.
pub fn admit_facts(
    tenant: &str,
    batch: &FactBatch,
    cache: &GroundingCache,
) -> Result<FactAdmission, String> {
    let mut deduper = FactDeduper::default();
    let mut admission = FactAdmission::default();
    for fact in &batch.facts {
        if !deduper.admit(fact) {
            admission.duplicates += 1;
            continue;
        }
        let claim: CandidateClaim = ground_fact(fact.clone(), cache);
        if matches!(claim.grounding, GroundingResult::Grounded { .. }) {
            admission.grounded += 1;
        }
        let digest = fact_digest(tenant, fact)?;
        let atom = KnowledgeAtom {
            atom_id: digest.clone(),
            kind: AtomKind::Fact,
            text: format!("{} {} {}", fact.subject, fact.predicate, fact.object),
            provenance_key: span_reference(fact),
            tenant: tenant.to_string(),
        };
        check_atom_provenance(&atom).map_err(|refusal| invalid(refusal.code()))?;
        let claim_id = format!("candidate_claim:{digest}");
        let atom_id = atom_node_id(&atom.atom_id);
        admission.methods.push(Method::AddNode {
            node_id: claim_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": CANDIDATE_CLAIM_NODE_TYPE,
                "tenant": tenant,
                "fact": claim.fact,
                "grounding": claim.grounding,
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
        let batch = FactBatch {
            facts: vec![
                // Subject and object swapped: direction repair grounds it.
                fact("Note G", "Ada Lovelace", 10),
                // The same triple again drops; an unknown entity stays a claim.
                fact("Note G", "Ada Lovelace", 10),
                fact("Somebody", "Something", 50),
            ],
        };
        let admission = admit_facts("t1", &batch, &cache).unwrap();
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
