//! Served meeting transcript revisions, summary contexts and summary commits
//! (MEDULLA-MEDIA-VOCAB-R001.101.3 / .4 / .5).
//!
//! Every function here is pure over the stored node properties it is handed:
//! the graph route reads the current `MeetingTranscript` (and `SummaryContext`)
//! node, calls one function, and commits the returned write-set through the one
//! ChangeEnvelope authority, exactly like `EnrichDocument`. A refusal returns
//! before any write-set exists, so a refused request leaves the store unchanged.

pub mod commit;
pub mod context;

use std::collections::BTreeMap;

use medulla_core::meeting_transcript::{apply_revision, get_final, TranscriptState};
use medulla_protocol_types::protocol::Method;
use medulla_types::meeting_transcript::{
    MeetingTranscriptRevision, MeetingTranscriptRevisionReceipt, TranscriptRevisionOp,
};

use super::repository_index::{stored_value, EnvelopeLabels, IndexWriteSet};

/// The labels every meeting transcript envelope is sealed under.
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "meeting-transcript-sha256",
    policy_version: "meeting-transcript",
};

/// Node type of the per-meeting transcript head (holds the revision state).
pub const TRANSCRIPT_TYPE: &str = "MeetingTranscript";
/// Node type of one transcript segment (`media-v1.ttl`).
pub const SEGMENT_TYPE: &str = "TranscriptSegment";
/// Node type of one committed revision event.
pub const REVISION_TYPE: &str = "TranscriptRevision";
/// The stored property holding the serialized [`TranscriptState`].
const STATE_PROPERTY: &str = "state_json";

/// The node id of a meeting's transcript head.
pub fn transcript_node_id(meeting_id: &str) -> String {
    format!("meeting-transcript:{meeting_id}")
}

/// The node id of one segment of a meeting.
pub fn segment_node_id(meeting_id: &str, segment_id: &str) -> String {
    format!("transcript-segment:{meeting_id}:{segment_id}")
}

/// Encode string properties as the ordered MessagePack map graph rows hold.
pub(crate) fn encode(properties: BTreeMap<&str, String>) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(&properties)
        .map_err(|error| format!("meeting transcript property encoding failed: {error}"))
}

/// Decode a stored row's string properties.
pub(crate) fn decode(properties: &[u8]) -> Result<BTreeMap<String, serde_json::Value>, String> {
    rmp_serde::from_slice(properties)
        .map_err(|error| format!("meeting transcript properties are invalid: {error}"))
}

/// The transcript state held by a `MeetingTranscript` row; an absent row is
/// the empty state of a new meeting.
pub fn load_state(properties: Option<&[u8]>) -> Result<TranscriptState, String> {
    let Some(properties) = properties else {
        return Ok(TranscriptState::default());
    };
    let map = decode(properties)?;
    let Some(serde_json::Value::String(json)) = map.get(STATE_PROPERTY) else {
        return Err("meeting transcript row holds no state".into());
    };
    serde_json::from_str(json)
        .map_err(|error| format!("meeting transcript state is invalid: {error}"))
}

/// The upsert of a meeting's transcript head holding `state`.
pub(crate) fn head_method(state: &TranscriptState) -> Result<Method, String> {
    let json = serde_json::to_string(state)
        .map_err(|error| format!("meeting transcript state encoding failed: {error}"))?;
    let mut properties = BTreeMap::new();
    properties.insert("type", TRANSCRIPT_TYPE.to_string());
    properties.insert("meeting_id", state.meeting_id.clone());
    properties.insert("revision", state.revision.to_string());
    if let Some(sealed) = get_final(state) {
        properties.insert("final_digest", sealed.finalization_digest.to_hex());
        properties.insert("final_revision", sealed.revision.to_string());
    }
    properties.insert(STATE_PROPERTY, json);
    Ok(Method::AddNode {
        node_id: transcript_node_id(&state.meeting_id),
        properties_msgpack: encode(properties)?,
    })
}

fn edge(source: String, target: String, relationship: &str) -> Result<Method, String> {
    let mut properties = BTreeMap::new();
    properties.insert("relationship", relationship.to_string());
    Ok(Method::AddEdge {
        source_id: source,
        target_id: target,
        properties_msgpack: encode(properties)?,
    })
}

fn segment_methods(event: &MeetingTranscriptRevision, head: &str) -> Result<Vec<Method>, String> {
    let mut methods = Vec::new();
    for segment_id in &event.ordered_segment_ids {
        let node_id = segment_node_id(&event.meeting_id, segment_id);
        if event.op == TranscriptRevisionOp::DeleteSegment {
            methods.push(Method::RemoveNode { node_id });
            continue;
        }
        let mut properties = BTreeMap::new();
        properties.insert("type", SEGMENT_TYPE.to_string());
        properties.insert("meeting_id", event.meeting_id.clone());
        properties.insert("segment_id", segment_id.clone());
        properties.insert("text_digest", event.text_digest.to_hex());
        properties.insert("start_ms", event.start_ms.to_string());
        properties.insert("end_ms", event.end_ms.to_string());
        properties.insert(
            "provenance_ref",
            stored_value(&event.provenance_ref).to_string(),
        );
        if let Some(speaker) = &event.speaker_ref {
            properties.insert("speaker_ref", stored_value(speaker).to_string());
        }
        methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(properties)?,
        });
        methods.push(edge(node_id, head.to_string(), "segmentOf")?);
    }
    Ok(methods)
}

/// Apply one revision event to the stored state and lower the result: the
/// updated head, the segment nodes and one `TranscriptRevision` node. An exact
/// replay answers the stored revision with no write-set; a conflict refuses by
/// its code before anything is lowered.
pub fn commit_revision(
    state: &TranscriptState,
    event: &MeetingTranscriptRevision,
) -> Result<(Option<IndexWriteSet>, MeetingTranscriptRevisionReceipt), String> {
    if !state.meeting_id.is_empty() && state.meeting_id != event.meeting_id {
        return Err("meeting transcript row belongs to another meeting".into());
    }
    let applied = apply_revision(state, event).map_err(|conflict| {
        format!(
            "{}: meeting '{}' revision {}",
            conflict.code(),
            event.meeting_id,
            event.base_revision
        )
    })?;
    let finalization_digest = (event.op == TranscriptRevisionOp::Finalize)
        .then(|| get_final(&applied.state).map(|sealed| sealed.finalization_digest))
        .flatten();
    let receipt = MeetingTranscriptRevisionReceipt {
        meeting_id: event.meeting_id.clone(),
        revision: applied.revision,
        replayed: applied.replayed,
        finalization_digest,
    };
    if applied.replayed {
        return Ok((None, receipt));
    }
    let head = transcript_node_id(&event.meeting_id);
    let mut methods = vec![head_method(&applied.state)?];
    methods.extend(segment_methods(event, &head)?);
    let revision_id = format!("{head}:revision:{}", applied.revision);
    let mut properties = BTreeMap::new();
    properties.insert("type", REVISION_TYPE.to_string());
    properties.insert("meeting_id", event.meeting_id.clone());
    properties.insert("revision", applied.revision.to_string());
    properties.insert(
        "op",
        serde_json::to_value(event.op)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default(),
    );
    properties.insert("sequence", event.monotonic_event_sequence.to_string());
    properties.insert(
        "idempotency_key",
        stored_value(&event.idempotency_key).to_string(),
    );
    properties.insert("text_digest", event.text_digest.to_hex());
    if let Some(digest) = finalization_digest {
        properties.insert("finalization_digest", digest.to_hex());
    }
    methods.push(Method::AddNode {
        node_id: revision_id.clone(),
        properties_msgpack: encode(properties)?,
    });
    methods.push(edge(revision_id, head, "revisionOf")?);
    let write_set = IndexWriteSet::from_methods(
        "meeting-transcript:",
        b"medulla/meeting-transcript-revision",
        methods,
    )?;
    Ok((Some(write_set), receipt))
}
