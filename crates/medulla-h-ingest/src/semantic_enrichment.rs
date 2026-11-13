//! Lowering of the served `SemanticEnrich` writes (MEDULLA-DECISION-ENGINE-R614.3).
//!
//! The pure behaviours live in `medulla_enrich::semantic`; this module turns
//! their results into one digest-keyed [`IndexWriteSet`] that commits through
//! the same ChangeEnvelope authority as repository and Document enrichment, so
//! a replay of an unchanged result commits nothing new.

use medulla_enrich::semantic::{CardNode, EnhancementCandidate, ProjectedEdge, TopicAssignment};
use medulla_protocol_types::protocol::Method;
use medulla_types::semantic_enrichment::ConceptInputWire;
use serde_json::{json, Value};

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

pub const CARD_TYPE: &str = "CapabilityCard";
pub const PROPOSAL_TYPE: &str = "EnhancementProposal";
pub const ASSIGNMENT_TYPE: &str = "TopicAssignment";
pub const HAS_CARD: &str = "hasCard";
pub const ASSIGNED_TO: &str = "assignedTo";
const ENVELOPE_PREFIX: &str = "semantic-enrichment:";
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "semantic-enrichment-sha256",
    policy_version: "semantic-enrichment",
};

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("semantic enrichment encoding failed: {error}"))
}

fn node(id: &str, properties: Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id.to_string(),
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

fn write_set(methods: Vec<Method>) -> Result<Option<IndexWriteSet>, String> {
    if methods.is_empty() {
        return Ok(None);
    }
    IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/semantic-enrichment", methods).map(Some)
}

/// One capability card: its node (the summary, model, prompt digest and cited
/// fact ids of R614.5) and the `hasCard` edge from its subject.
pub fn lower_card(
    card: &CardNode,
    schedule_record_id: &str,
) -> Result<Option<IndexWriteSet>, String> {
    let mut properties = json!({ "type": CARD_TYPE, "schedule_record_id": schedule_record_id });
    for (key, value) in &card.properties {
        properties[key] = json!(value);
    }
    properties["source_fact_ids"] = json!(card
        .properties
        .get("source_fact_ids")
        .map(|joined| joined.split(',').collect::<Vec<_>>())
        .unwrap_or_default());
    write_set(vec![
        node(card.id.as_str(), properties)?,
        edge(card.subject.as_str(), card.id.as_str(), HAS_CARD)?,
    ])
}

/// The id of the proposal node of one concept under one subject.
pub fn proposal_id(subject: &str, concept: &str) -> String {
    let digest =
        sha256_hex(format!("{}:{subject}\0{}:{concept}", subject.len(), concept.len()).as_bytes());
    format!("proposal:{digest}")
}

/// The ranked proposals: one node each, with the inputs and the schedule
/// record id of R614.5.
pub fn lower_proposals(
    subject: &str,
    ranked: &[EnhancementCandidate],
    inputs: &[ConceptInputWire],
    schedule_record_id: &str,
) -> Result<Option<IndexWriteSet>, String> {
    let mut methods = Vec::with_capacity(ranked.len());
    for candidate in ranked {
        let input = inputs
            .iter()
            .find(|input| input.concept == candidate.concept.as_str())
            .ok_or_else(|| format!("ranked concept {} has no input", candidate.concept))?;
        methods.push(node(
            &proposal_id(subject, candidate.concept.as_str()),
            json!({
                "type": PROPOSAL_TYPE,
                "subject": subject,
                "concept": candidate.concept.as_str(),
                "value": candidate.value,
                "impact_bp": input.impact_bp,
                "confidence_bp": input.confidence_bp,
                "cost_units": input.cost_units,
                "schedule_record_id": schedule_record_id,
            }),
        )?);
    }
    write_set(methods)
}

/// The id of the assignment node of one document under one taxonomy.
pub fn assignment_id(document: &str, taxonomy: &str) -> String {
    format!(
        "assignment:{}",
        sha256_hex(format!("{document}\0{taxonomy}").as_bytes())
    )
}

/// One topic assignment: its node (taxonomy, topic, confidence and embedder)
/// and the `assignedTo` edge from the document to the topic.
pub fn lower_assignment(
    assignment: &TopicAssignment,
    taxonomy: &str,
    embedder: &str,
    schedule_record_id: &str,
) -> Result<Option<IndexWriteSet>, String> {
    let id = assignment_id(assignment.document.as_str(), taxonomy);
    write_set(vec![
        node(
            &id,
            json!({
                "type": ASSIGNMENT_TYPE,
                "document": assignment.document.as_str(),
                "taxonomy": taxonomy,
                "topic": assignment.topic.as_str(),
                "confidence_bp": assignment.confidence.basis_points(),
                "embedder": embedder,
                "schedule_record_id": schedule_record_id,
            }),
        )?,
        edge(
            assignment.document.as_str(),
            assignment.topic.as_str(),
            ASSIGNED_TO,
        )?,
    ])
}

/// The projected edges, one `AddEdge` each.
pub fn lower_projection(edges: &[ProjectedEdge]) -> Result<Option<IndexWriteSet>, String> {
    let methods = edges
        .iter()
        .map(|projected| {
            edge(
                projected.source.as_str(),
                projected.target.as_str(),
                projected.relation.as_str(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    write_set(methods)
}
