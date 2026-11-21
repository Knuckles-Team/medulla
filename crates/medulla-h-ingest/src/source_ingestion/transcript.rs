//! Transcript validation: source order, both clocks and source identity
//! (MEDULLA-INGEST-WORKER-R003.14).
//!
//! `validate_transcript` checks one [`TranscriptThread`] and its messages
//! against the source request they came from. Messages keep the order of
//! `TranscriptThread.message_ids`: the validator never sorts. The event time
//! and the storage time of a message pass through apart. Identities are
//! derived here, never read from the payload. Line reading and checkpoints
//! belong to MEDULLA-REPO-INGEST-R800.
//!
//! A transcripts batch carries typed items, not mapped records
//! (MEDULLA-INGEST-WORKER-R003.15): [`decode_transcript_items`] reads them,
//! [`validate_transcript`] checks them and [`transcript_methods`] writes the thread, its
//! messages and their edges.

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use medulla_document::TranscriptDialect;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::{SourceIngestionRequest, SourceJson};
use medulla_source_types::source_transcript::{
    TranscriptIngestError, TranscriptItem, TranscriptMessage, TranscriptThread, SEGMENT_CLOCKS,
};
use medulla_types::contract::Digest256;

/// The digest domain of a thread identity.
const THREAD_DOMAIN: &[u8] = b"eg/transcript-thread";
/// The digest domain of a message identity.
const MESSAGE_DOMAIN: &[u8] = b"eg/transcript-message";

/// One validated message with the identity the engine derived for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedMessage {
    pub message_id: String,
    pub message: TranscriptMessage,
}

/// A validated thread: its derived identity and its messages in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedTranscript {
    pub thread_id: String,
    pub messages: Vec<ValidatedMessage>,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("TRANSCRIPT_INVALID: {detail}")
}

/// `source_identity` of a thread: SHA-256 framed under `eg/transcript-thread`
/// over tenant, connector, stream and session id.
pub(super) fn thread_identity(
    tenant: &str,
    connector: &str,
    stream: &str,
    session_id: &str,
) -> Result<String, String> {
    Ok(Digest256::framed(
        THREAD_DOMAIN,
        &[
            tenant.as_bytes(),
            connector.as_bytes(),
            stream.as_bytes(),
            session_id.as_bytes(),
        ],
    )?
    .to_hex())
}

/// `source_identity` of a message: SHA-256 framed under
/// `eg/transcript-message` over the thread id and the source message id.
pub(super) fn message_identity(thread_id: &str, message_id: &str) -> Result<String, String> {
    Ok(Digest256::framed(
        MESSAGE_DOMAIN,
        &[thread_id.as_bytes(), message_id.as_bytes()],
    )?
    .to_hex())
}

/// The edge from a message to its thread.
const TRANSCRIPT_OF: &str = "transcriptOf";
/// The edge from a message to the next one of its thread.
const PRECEDES: &str = "precedes";
/// The edge from a message to a concept it mentions (MEDULLA-INGEST-WORKER-R003.20).
const MENTIONS_CONCEPT: &str = "mentionsConcept";

/// A decoded transcripts batch: the thread header and its messages in the order sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DecodedTranscript {
    pub dialect: TranscriptDialect,
    pub thread: TranscriptThread,
    pub messages: Vec<TranscriptMessage>,
}

impl DecodedTranscript {
    /// Every distinct concept id the messages name.
    pub(super) fn concept_ids(&self) -> BTreeSet<String> {
        self.messages
            .iter()
            .flat_map(|message| message.concept_refs.iter().cloned())
            .collect()
    }
}

/// Decode the items of a transcripts batch: item 0 is the one `thread` item and every later item
/// is a `message` item. An undecodable item, and a missing, repeated or misplaced thread item,
/// are `TRANSCRIPT_INVALID`; the declared format goes through [`parse_dialect`].
pub(super) fn decode_transcript_items(items: &[SourceJson]) -> Result<DecodedTranscript, String> {
    let mut decoded = Vec::with_capacity(items.len());
    for item in items {
        let value = item.value().map_err(invalid)?;
        let item: TranscriptItem = serde_json::from_value(value).map_err(invalid)?;
        decoded.push(item);
    }
    let mut decoded = decoded.into_iter();
    let Some(TranscriptItem::Thread(header)) = decoded.next() else {
        return Err(invalid("item 0 must be the thread item"));
    };
    let dialect = parse_dialect(&header.dialect).map_err(|error| error.to_string())?;
    let mut messages = Vec::new();
    for item in decoded {
        match item {
            TranscriptItem::Message(message) => messages.push(message),
            TranscriptItem::Thread(_) => {
                return Err(invalid("a batch carries exactly one thread item"));
            }
        }
    }
    Ok(DecodedTranscript {
        dialect,
        thread: header.thread,
        messages,
    })
}

/// The methods of one validated transcripts batch, in write order.
#[derive(Debug, Default)]
pub(super) struct TranscriptAdmission {
    /// Nodes first, then edges.
    pub methods: Vec<Method>,
    /// The nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// The `mentionsConcept` edges written.
    pub concept_link_count: u64,
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| invalid(format!("encoding failed: {error}")))
}

fn edge(source: &str, target: &str, relationship: &str) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source.to_string(),
        target_id: target.to_string(),
        properties_msgpack: encode(&serde_json::json!({ "relationship": relationship }))?,
    })
}

/// `transcript_methods`: node `transcript-thread:<thread_id>` of type `Transcript`, one node
/// `transcript-message:<message_id>` of type `TranscriptSegment` per message, and the edges
/// `transcriptOf` (message to thread), `precedes` (message to the next) and `mentionsConcept`
/// (message to each concept ref). `sequence` is the 0-based position in the thread; an absent
/// `narrative_time_ms` stays absent.
pub(super) fn transcript_methods(
    decoded: &DecodedTranscript,
    validated: &ValidatedTranscript,
) -> Result<TranscriptAdmission, String> {
    let thread_node = format!("transcript-thread:{}", validated.thread_id);
    let mut admission = TranscriptAdmission::default();
    admission.methods.push(Method::AddNode {
        node_id: thread_node.clone(),
        properties_msgpack: encode(&serde_json::json!({
            "type": "Transcript",
            "dialect": serde_json::to_value(decoded.dialect).map_err(invalid)?,
            "connector": decoded.thread.connector.as_str(),
            "stream": decoded.thread.stream.as_str(),
            "session_id": decoded.thread.session_id,
            "message_count": validated.messages.len(),
        }))?,
    });
    admission.node_ids.push(thread_node.clone());
    let mut message_nodes = Vec::with_capacity(validated.messages.len());
    for (sequence, validated_message) in validated.messages.iter().enumerate() {
        let message = &validated_message.message;
        let node_id = format!("transcript-message:{}", validated_message.message_id);
        let mut properties = serde_json::Map::new();
        properties.insert("type".into(), "TranscriptSegment".into());
        properties.insert(
            "role".into(),
            serde_json::to_value(message.role).map_err(invalid)?,
        );
        properties.insert("text".into(), message.text.clone().into());
        properties.insert("sequence".into(), sequence.into());
        properties.insert(
            "source_record_id".into(),
            message.provenance.source_record_id.clone().into(),
        );
        properties.insert(
            "source_uri".into(),
            message.provenance.source_uri.clone().into(),
        );
        // The clocks named by `SEGMENT_CLOCKS`, in its order: storage, event, narrative.
        let clocks = [
            Some(message.storage_time_ms),
            Some(message.event_time_ms),
            message.narrative_time_ms,
        ];
        for ((property, _), value) in SEGMENT_CLOCKS.iter().zip(clocks) {
            if let Some(value) = value {
                properties.insert((*property).into(), value.into());
            }
        }
        admission.methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(&serde_json::Value::Object(properties))?,
        });
        admission.node_ids.push(node_id.clone());
        message_nodes.push(node_id);
    }
    for (index, node_id) in message_nodes.iter().enumerate() {
        admission
            .methods
            .push(edge(node_id, &thread_node, TRANSCRIPT_OF)?);
        if let Some(next) = message_nodes.get(index + 1) {
            admission.methods.push(edge(node_id, next, PRECEDES)?);
        }
        for concept in &validated.messages[index].message.concept_refs {
            admission
                .methods
                .push(edge(node_id, concept, MENTIONS_CONCEPT)?);
            admission.concept_link_count += 1;
        }
    }
    Ok(admission)
}

/// `parse_dialect`: the declared transcript format of a thread header. Only a name in
/// `TranscriptDialect::ALL` is accepted (MEDULLA-INGEST-WORKER-R003.16); the dialect is a
/// provenance label because the producer has already parsed the source lines.
pub(super) fn parse_dialect(name: &str) -> Result<TranscriptDialect, TranscriptIngestError> {
    TranscriptDialect::ALL
        .iter()
        .copied()
        .find(|dialect| {
            serde_json::to_value(dialect)
                .ok()
                .and_then(|value| value.as_str().map(|text| text == name))
                .unwrap_or(false)
        })
        .ok_or_else(|| TranscriptIngestError::UnknownFormat(name.to_string()))
}

/// Validate `thread` and `messages` against `request`.
///
/// `tenant` is the verified tenant scope. `concept_visible` answers whether a
/// concept reference resolves to a node visible under the verified tenant and
/// policy; a reference that does not resolve refuses the whole thread with
/// `TRANSCRIPT_FOREIGN_CONCEPT`. An `event_time_ms` of zero or below refuses with
/// `TRANSCRIPT_MISSING_EVENT_TIME`. A message id sent twice with different content refuses with
/// `TRANSCRIPT_DUPLICATE_MESSAGE_CONFLICT`; the same content is an idempotent no-op
/// (MEDULLA-INGEST-WORKER-R003.16). The messages returned follow `thread.message_ids` exactly.
pub(super) fn validate_transcript(
    tenant: &str,
    request: &SourceIngestionRequest,
    thread: &TranscriptThread,
    messages: &[TranscriptMessage],
    concept_visible: impl Fn(&str) -> bool,
) -> Result<ValidatedTranscript, String> {
    thread.validate_against(request).map_err(invalid)?;
    let mut by_id: BTreeMap<&str, &TranscriptMessage> = BTreeMap::new();
    for message in messages {
        message.validate_against(request).map_err(invalid)?;
        if message.session_id != thread.session_id {
            return Err(invalid(format!(
                "message {} belongs to session {}, not {}",
                message.message_id, message.session_id, thread.session_id
            )));
        }
        if message.event_time_ms <= 0 {
            return Err(
                TranscriptIngestError::MissingEventTime(message.message_id.clone()).to_string(),
            );
        }
        if let Some(earlier) = by_id.insert(message.message_id.as_str(), message) {
            if earlier != message {
                return Err(TranscriptIngestError::DuplicateMessageConflict(
                    message.message_id.clone(),
                )
                .to_string());
            }
        }
    }
    let thread_id = thread_identity(
        tenant,
        thread.connector.as_str(),
        thread.stream.as_str(),
        &thread.session_id,
    )?;
    let mut ordered = Vec::with_capacity(thread.message_ids.len());
    for id in &thread.message_ids {
        let message = by_id
            .remove(id.as_str())
            .ok_or_else(|| invalid(format!("thread names message {id} that was not sent")))?;
        if let Some(concept) = message
            .concept_refs
            .iter()
            .find(|concept| !concept_visible(concept))
        {
            return Err(TranscriptIngestError::ForeignConcept(concept.clone()).to_string());
        }
        ordered.push(ValidatedMessage {
            message_id: message_identity(&thread_id, id)?,
            message: message.clone(),
        });
    }
    if let Some(extra) = by_id.keys().next() {
        return Err(invalid(format!("message {extra} is not in the thread")));
    }
    Ok(ValidatedTranscript {
        thread_id,
        messages: ordered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_transcript::{TranscriptProvenance, TranscriptRole};

    fn request() -> SourceIngestionRequest {
        medulla_source_types::source_ingestion::fixtures::request()
    }

    fn message(request: &SourceIngestionRequest, id: &str, event: i64, storage: i64) -> TranscriptMessage {
        let batch = request.as_batch();
        TranscriptMessage {
            connector: batch.connector.clone(),
            stream: batch.provider_checkpoint.stream.clone(),
            session_id: "session-1".into(),
            message_id: id.into(),
            role: TranscriptRole::User,
            event_time_ms: event,
            storage_time_ms: storage,
            narrative_time_ms: None,
            text: format!("text of {id}"),
            concept_refs: vec!["concept-a".into()],
            provenance: TranscriptProvenance {
                source_record_id: format!("record-{id}"),
                source_uri: format!("demo:session-1:{id}"),
            },
        }
    }

    fn thread(request: &SourceIngestionRequest, ids: &[&str]) -> TranscriptThread {
        let batch = request.as_batch();
        TranscriptThread {
            connector: batch.connector.clone(),
            stream: batch.provider_checkpoint.stream.clone(),
            session_id: "session-1".into(),
            message_ids: ids.iter().map(|id| id.to_string()).collect(),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R003.14
    #[test]
    fn source_sequence_and_identity_are_preserved() {
        let request = request();
        // Sent out of order and with event times that are not increasing: the thread order
        // wins and nothing is sorted.
        let sent = vec![
            message(&request, "m3", 300, 3),
            message(&request, "m1", 900, 1),
            message(&request, "m2", 100, 2),
        ];
        let thread = thread(&request, &["m1", "m2", "m3"]);
        let validated =
            validate_transcript("t1", &request, &thread, &sent, |c| c == "concept-a").unwrap();
        let order: Vec<&str> = validated
            .messages
            .iter()
            .map(|m| m.message.message_id.as_str())
            .collect();
        assert_eq!(order, ["m1", "m2", "m3"]);
        // Both clocks pass through apart.
        assert_eq!(validated.messages[0].message.event_time_ms, 900);
        assert_eq!(validated.messages[0].message.storage_time_ms, 1);
        assert_eq!(
            validated.messages[0].message.provenance.source_record_id,
            "record-m1"
        );
        // Identities are derived, 64 hex characters, and distinct per message.
        assert_eq!(validated.thread_id.len(), 64);
        assert_ne!(validated.messages[0].message_id, validated.messages[1].message_id);
        assert_eq!(
            validated.messages[0].message_id,
            message_identity(&validated.thread_id, "m1").unwrap()
        );
        // The same session and message ids under another connector, stream or tenant
        // stay distinct.
        let base = thread_identity("t1", "c1", "s1", "session-1").unwrap();
        for other in [
            thread_identity("t2", "c1", "s1", "session-1").unwrap(),
            thread_identity("t1", "c2", "s1", "session-1").unwrap(),
            thread_identity("t1", "c1", "s2", "session-1").unwrap(),
        ] {
            assert_ne!(base, other);
            assert_ne!(
                message_identity(&base, "m1").unwrap(),
                message_identity(&other, "m1").unwrap()
            );
        }
        // Validation is repeatable.
        assert_eq!(
            validated,
            validate_transcript("t1", &request, &thread, &sent, |c| c == "concept-a").unwrap()
        );

        // A concept the caller cannot see refuses the thread.
        let error = validate_transcript("t1", &request, &thread, &sent, |_| false).unwrap_err();
        assert!(error.starts_with("TRANSCRIPT_FOREIGN_CONCEPT:"), "{error}");
        // A message the thread does not name, or names but is missing, is refused.
        let missing = validate_transcript("t1", &request, &thread, &sent[..2], |_| true);
        assert!(missing.unwrap_err().starts_with("TRANSCRIPT_INVALID:"));
        let short = self::thread(&request, &["m1", "m2"]);
        assert!(validate_transcript("t1", &request, &short, &sent, |_| true).is_err());
    }

    // spec: MEDULLA-INGEST-WORKER-R003.16
    #[test]
    fn unknown_format_and_foreign_concept_are_refused() {
        // Only a name of `TranscriptDialect::ALL` is a format.
        for name in ["antigravity", "windsurf", "claude", "codex"] {
            assert!(parse_dialect(name).is_ok(), "{name}");
        }
        for name in ["", "Claude", "plain-text"] {
            let refused = parse_dialect(name).unwrap_err();
            assert_eq!(refused, TranscriptIngestError::UnknownFormat(name.into()));
            assert_eq!(refused.code(), "TRANSCRIPT_UNKNOWN_FORMAT");
        }

        let request = request();
        let thread = thread(&request, &["m1", "m2"]);
        let sent = vec![
            message(&request, "m1", 100, 1),
            message(&request, "m2", 200, 2),
        ];
        // A concept the caller cannot see refuses the whole thread.
        let foreign = validate_transcript("t1", &request, &thread, &sent, |_| false).unwrap_err();
        assert!(foreign.starts_with("TRANSCRIPT_FOREIGN_CONCEPT:"), "{foreign}");

        // An event time of zero or below is missing.
        for event in [0, -5] {
            let mut bad = sent.clone();
            bad[1].event_time_ms = event;
            let refused = validate_transcript("t1", &request, &thread, &bad, |_| true).unwrap_err();
            assert!(refused.starts_with("TRANSCRIPT_MISSING_EVENT_TIME:"), "{refused}");
        }

        // A message id sent again with the same content is an idempotent no-op ...
        let mut same = sent.clone();
        same.push(sent[0].clone());
        let validated = validate_transcript("t1", &request, &thread, &same, |_| true).unwrap();
        assert_eq!(validated.messages.len(), 2);
        assert_eq!(
            validated,
            validate_transcript("t1", &request, &thread, &sent, |_| true).unwrap()
        );
        // ... and with different content it is a conflict.
        let mut changed = sent.clone();
        let mut other = sent[0].clone();
        other.text = "different text".into();
        changed.push(other);
        let conflict = validate_transcript("t1", &request, &thread, &changed, |_| true).unwrap_err();
        assert!(
            conflict.starts_with("TRANSCRIPT_DUPLICATE_MESSAGE_CONFLICT:"),
            "{conflict}"
        );
    }

    fn items(request: &SourceIngestionRequest, dialect: &str, ids: &[&str]) -> Vec<SourceJson> {
        let mut items = vec![SourceJson::new(serde_json::json!({
            "thread": {
                "dialect": dialect,
                "thread": thread(request, ids),
            }
        }))
        .unwrap()];
        for (index, id) in ids.iter().enumerate() {
            let mut sent = message(request, id, 100 + index as i64, 1 + index as i64);
            if index == 0 {
                sent.narrative_time_ms = Some(7);
            }
            items.push(
                SourceJson::new(serde_json::json!({ "message": sent })).unwrap(),
            );
        }
        items
    }

    // spec: MEDULLA-INGEST-WORKER-R003.15
    #[test]
    fn prepare_routes_declared_transcript_format() {
        let request = request();
        for dialect in TranscriptDialect::ALL {
            let name = serde_json::to_value(dialect).unwrap();
            let name = name.as_str().unwrap();
            let decoded =
                decode_transcript_items(&items(&request, name, &["m1", "m2", "m3"])).unwrap();
            assert_eq!(decoded.dialect, *dialect);
            assert_eq!(decoded.messages.len(), 3);
            assert_eq!(
                decoded.concept_ids(),
                BTreeSet::from(["concept-a".to_string()])
            );
            let validated = validate_transcript(
                "t1",
                &request,
                &decoded.thread,
                &decoded.messages,
                |id| id == "concept-a",
            )
            .unwrap();
            let admission = transcript_methods(&decoded, &validated).unwrap();
            // One thread node, three message nodes, then three transcriptOf, two precedes
            // and three mentionsConcept edges.
            let nodes: Vec<&Method> = admission
                .methods
                .iter()
                .filter(|method| matches!(method, Method::AddNode { .. }))
                .collect();
            assert_eq!(nodes.len(), 4);
            assert_eq!(admission.node_ids.len(), 4);
            assert_eq!(admission.concept_link_count, 3);
            let relationships: Vec<String> = admission
                .methods
                .iter()
                .filter_map(|method| match method {
                    Method::AddEdge {
                        properties_msgpack, ..
                    } => {
                        let props: serde_json::Value =
                            rmp_serde::from_slice(properties_msgpack).unwrap();
                        Some(props["relationship"].as_str().unwrap().to_string())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(relationships.len(), 8);
            for (relationship, count) in [("transcriptOf", 3), ("precedes", 2), ("mentionsConcept", 3)]
            {
                assert_eq!(
                    relationships.iter().filter(|r| *r == relationship).count(),
                    count,
                    "{relationship}"
                );
            }
            // Message nodes carry the segment type, sequence, provenance and clocks; an absent
            // narrative time stays absent.
            let Method::AddNode {
                node_id,
                properties_msgpack,
            } = &admission.methods[1]
            else {
                panic!("the first message node follows the thread node");
            };
            assert!(node_id.starts_with("transcript-message:"));
            let props: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
            assert_eq!(props["type"], "TranscriptSegment");
            assert_eq!(props["sequence"], 0);
            assert_eq!(props["source_record_id"], "record-m1");
            assert_eq!(props["event_time"], 100);
            assert_eq!(props["storage_time"], 1);
            assert_eq!(props["narrative_time"], 7);
            let Method::AddNode {
                properties_msgpack, ..
            } = &admission.methods[2]
            else {
                panic!("the second message node");
            };
            let second: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
            assert_eq!(second["sequence"], 1);
            assert!(second.get("narrative_time").is_none());
        }

        // An unknown format and an item 0 that is a message are refused.
        let unknown = decode_transcript_items(&items(&request, "plain-text", &["m1"])).unwrap_err();
        assert!(unknown.starts_with("TRANSCRIPT_UNKNOWN_FORMAT:"), "{unknown}");
        let mut misplaced = items(&request, "claude", &["m1", "m2"]);
        misplaced.swap(0, 1);
        let refused = decode_transcript_items(&misplaced).unwrap_err();
        assert!(refused.starts_with("TRANSCRIPT_INVALID:"), "{refused}");
        // A second thread item, an undecodable item and an empty batch are refused too.
        let mut twice = items(&request, "claude", &["m1"]);
        twice.push(twice[0].clone());
        assert!(decode_transcript_items(&twice)
            .unwrap_err()
            .starts_with("TRANSCRIPT_INVALID:"));
        let garbage = vec![SourceJson::new(serde_json::json!({"nonsense": 1})).unwrap()];
        assert!(decode_transcript_items(&garbage)
            .unwrap_err()
            .starts_with("TRANSCRIPT_INVALID:"));
        assert!(decode_transcript_items(&[])
            .unwrap_err()
            .starts_with("TRANSCRIPT_INVALID:"));
    }
}
