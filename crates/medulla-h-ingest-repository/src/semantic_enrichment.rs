//! Lowering of the served `SemanticEnrich` writes (MEDULLA-DECISION-ENGINE-R614.3).
//!
//! The pure behaviours live in `medulla_enrich::semantic`; this module turns
//! their results into one digest-keyed [`IndexWriteSet`] that commits through
//! the same ChangeEnvelope authority as repository and Document enrichment, so
//! a replay of an unchanged result commits nothing new.

use medulla_enrich::semantic::{
    CardNode, EnhancementCandidate, FittedTopics, ProjectedEdge, TopicAssignment, TopicCentroid,
};
use medulla_enrich::{EnrichError, Id};
use medulla_protocol_types::protocol::Method;
use medulla_types::semantic_enrichment::ConceptInputWire;
use serde_json::{json, Value};

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

pub const CARD_TYPE: &str = "CapabilityCard";
pub const PROPOSAL_TYPE: &str = "EnhancementProposal";
pub const ASSIGNMENT_TYPE: &str = "TopicAssignment";
/// The label of a stored topic node (MEDULLA-DECISION-ENGINE-R614.2.6).
pub const TOPIC_TYPE: &str = "Topic";
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

/// One whole-document node write: `AddNode` stores `properties` as the node's complete property
/// document, so a node already stored under `id` loses every property the new document does not
/// name. This is the replace form, as opposed to a merge (`BatchUpdate` `upsert_node`, or
/// `CompareAndSetNodeFields`, which keep the fields they do not name). A writer that must not
/// leave a stale field behind calls this and never `node`.
fn replace_node(id: &str, properties: Value) -> Result<Method, String> {
    node(id, properties)
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

/// The fitted topics of one taxonomy (MEDULLA-DECISION-ENGINE-R614.2.6): one `Topic` node per
/// topic, keyed by the topic id, so a refit of the taxonomy replaces the earlier node, with the
/// confidence floor and hold-out precision as properties and the centroid as the node's
/// embedding, which is where `classify` reads it. The write set is the one ingest route; no
/// second write path exists.
pub fn lower_fit(
    fit: &FittedTopics,
    taxonomy: &str,
    schedule_record_id: &str,
) -> Result<Option<IndexWriteSet>, String> {
    let mut methods = Vec::with_capacity(fit.topics.len() * 2);
    for topic in &fit.topics {
        // A refit of one taxonomy replaces the stored topic document whole: a field the first
        // fit wrote and this one does not (a floor, a hold-out count) cannot survive it.
        methods.push(replace_node(
            topic.topic.as_str(),
            json!({
                "type": TOPIC_TYPE,
                "taxonomy": taxonomy,
                "floor_bp": topic.floor_bp,
                "holdout_precision_bp": topic.holdout_precision_bp,
                "holdout_bp": fit.holdout_bp,
                "train_examples": topic.train_examples,
                "holdout_examples": topic.holdout_examples.len(),
                "schedule_record_id": schedule_record_id,
            }),
        )?);
        methods.push(Method::AddEmbedding {
            node_id: topic.topic.as_str().to_string(),
            embedding: topic.centroid.clone(),
        });
    }
    write_set(methods)
}

/// The topics of `taxonomy` among stored `Topic` nodes: each node row as its decoded properties,
/// with its centroid read by `embedding_of`. A node of another taxonomy, or without a centroid,
/// is skipped. A `floor_bp` property, written by a fit, becomes the topic's confidence floor.
/// The caller passes only rows the reader may see; the result is in topic id order.
pub fn stored_topics(
    rows: impl IntoIterator<Item = (String, Value)>,
    embedding_of: impl Fn(&str) -> Option<Vec<f32>>,
    taxonomy: &str,
) -> Result<Vec<TopicCentroid>, EnrichError> {
    let mut topics = Vec::new();
    for (node_id, properties) in rows {
        if properties.get("taxonomy").and_then(Value::as_str) != Some(taxonomy) {
            continue;
        }
        let Some(centroid) = embedding_of(&node_id) else {
            continue;
        };
        let floor_bp = properties
            .get("floor_bp")
            .and_then(Value::as_u64)
            .and_then(|floor| u32::try_from(floor).ok());
        topics.push(TopicCentroid {
            topic: Id::new(node_id)?,
            centroid,
            floor_bp,
        });
    }
    topics.sort_by(|a, b| a.topic.cmp(&b.topic));
    Ok(topics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fitted(floor_bp: u32) -> FittedTopics {
        FittedTopics {
            holdout_bp: 2000,
            topics: vec![medulla_enrich::semantic::FittedTopic {
                topic: Id::new("topic:billing").unwrap(),
                centroid: vec![1.0, 0.0],
                holdout_precision_bp: 9000,
                floor_bp,
                train_examples: 8,
                holdout_examples: vec![Id::new("e1").unwrap()],
            }],
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R614.2.6
    #[test]
    fn a_refit_writes_the_topic_as_one_whole_document_replace() {
        let first = lower_fit(&fitted(7000), "billing", "sched-1")
            .unwrap()
            .unwrap();
        let second = lower_fit(&fitted(8000), "billing", "sched-2")
            .unwrap()
            .unwrap();
        let document = |set: &IndexWriteSet| match &set.methods()[0] {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => (
                node_id.clone(),
                rmp_serde::from_slice::<Value>(properties_msgpack).unwrap(),
            ),
            other => panic!("the topic is written by one AddNode, got {other:?}"),
        };
        let (first_id, first_doc) = document(&first);
        let (second_id, second_doc) = document(&second);
        // Both fits address the same node id, so the second replaces the first.
        assert_eq!(first_id, second_id);
        assert_eq!(first_doc["floor_bp"], 7000);
        assert_eq!(second_doc["floor_bp"], 8000);
        // The document is complete: the same property keys under both fits.
        let keys = |doc: &Value| doc.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&first_doc), keys(&second_doc));
        // The centroid follows the node write as its embedding.
        assert!(matches!(second.methods()[1], Method::AddEmbedding { .. }));
    }
}
