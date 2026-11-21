//! Transcript validation: source order, both clocks and source identity
//! (MEDULLA-INGEST-WORKER-R003.14).
//!
//! `validate_transcript` checks one [`TranscriptThread`] and its messages
//! against the source request they came from. Messages keep the order of
//! `TranscriptThread.message_ids`: the validator never sorts. The event time
//! and the storage time of a message pass through apart. Identities are
//! derived here, never read from the payload. Line reading and checkpoints
//! belong to MEDULLA-REPO-INGEST-R800.

// The caller is the `prepare` wiring of MEDULLA-INGEST-WORKER-R003.15.
#![allow(dead_code)]

use std::collections::BTreeMap;

use medulla_source_types::source_ingestion::SourceIngestionRequest;
use medulla_source_types::source_transcript::{TranscriptMessage, TranscriptThread};
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

/// Validate `thread` and `messages` against `request`.
///
/// `tenant` is the verified tenant scope. `concept_visible` answers whether a
/// concept reference resolves to a node visible under the verified tenant and
/// policy; a reference that does not resolve refuses the whole thread with
/// `TRANSCRIPT_FOREIGN_CONCEPT`. The messages returned follow
/// `thread.message_ids` exactly.
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
        if by_id
            .insert(message.message_id.as_str(), message)
            .is_some()
        {
            return Err(invalid(format!(
                "message {} appears more than once",
                message.message_id
            )));
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
            return Err(format!(
                "TRANSCRIPT_FOREIGN_CONCEPT: concept {concept} is not visible to the caller"
            ));
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
}
