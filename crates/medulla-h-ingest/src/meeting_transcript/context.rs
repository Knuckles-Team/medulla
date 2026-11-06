//! `MeetingSummaryContext`: the bounded context a summary is produced from
//! (MEDULLA-MEDIA-VOCAB-R001.101.4).
//!
//! The context is cut from the current finalized transcript under a
//! `MeetingSummaryLimitPolicy` whose limits are positive and at most their
//! fixed ceilings. One `SummaryContext` node keyed by `context_digest` records
//! the admission the later receipt must match. Every refusal returns before a
//! write-set exists.

use std::collections::BTreeMap;

use medulla_core::meeting_transcript::{get_final, TranscriptState};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use medulla_types::meeting_transcript::{
    MeetingSummaryContext, MeetingSummaryContextRequest, SummarySpan,
};

use super::{encode, transcript_node_id};
use crate::repository_index::IndexWriteSet;

/// Node type of one stored summary context.
pub const CONTEXT_TYPE: &str = "SummaryContext";

/// The node id of the context of `context_digest`.
pub fn context_node_id(context_digest: &Digest256) -> String {
    format!("summary-context:{}", context_digest.to_hex())
}

/// The span bounds stored on the context node: `(span_id, segment_id, bytes)`.
pub type StoredSpan = (String, String, u32);

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The finalized digest a context or summary may be built from: the latest
/// seal, and only while no revision followed it.
pub(crate) fn current_final(
    state: &TranscriptState,
    digest: &Digest256,
) -> Result<Vec<String>, String> {
    let Some(sealed) = get_final(state) else {
        return Err(refuse(
            "TRANSCRIPT_NOT_FINAL",
            format!("meeting '{}' has no finalized revision", state.meeting_id),
        ));
    };
    if sealed.finalization_digest != *digest || sealed.revision != state.revision {
        return Err(refuse(
            "TRANSCRIPT_STALE",
            format!(
                "meeting '{}' digest is not the current finalized digest",
                state.meeting_id
            ),
        ));
    }
    Ok(sealed.ordered_segment_ids.clone())
}

/// The longest prefix of `text` of at most `max_bytes` bytes on a char boundary.
fn clip(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whitespace-delimited token count.
fn tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Build the bounded context and the write-set of its `SummaryContext` node.
pub fn build_context(
    state: &TranscriptState,
    request: &MeetingSummaryContextRequest,
) -> Result<(IndexWriteSet, MeetingSummaryContext), String> {
    request
        .policy
        .check()
        .map_err(|code| refuse(code, "summary limit policy"))?;
    let Some(admission) = &request.admission else {
        return Err(refuse(
            "SUMMARY_ADMISSION_MISSING",
            "the request carries no admission reference",
        ));
    };
    if state.meeting_id.is_empty() {
        return Err(refuse(
            "TRANSCRIPT_NOT_FINAL",
            format!("meeting '{}' has no transcript", request.meeting_id),
        ));
    }
    let ordered = current_final(state, &request.transcript_digest)?;
    let digests: BTreeMap<&str, Digest256> = state
        .segments
        .iter()
        .map(|(id, digest)| (id.as_str(), *digest))
        .collect();
    let texts: BTreeMap<&str, &str> = request
        .segments
        .iter()
        .map(|segment| (segment.segment_id.as_str(), segment.text.as_str()))
        .collect();
    let policy = request.policy;
    let mut spans = Vec::new();
    let mut token_total = 0usize;
    for segment_id in ordered.iter().take(policy.max_spans as usize) {
        let text = texts
            .get(segment_id.as_str())
            .copied()
            .filter(|text| {
                digests.get(segment_id.as_str()) == Some(&Digest256::sha256(text.as_bytes()))
            })
            .ok_or_else(|| {
                refuse(
                    "TRANSCRIPT_STALE",
                    format!("segment '{segment_id}' text does not match the finalized digest"),
                )
            })?;
        let clipped = clip(text, policy.max_segment_bytes as usize);
        let count = tokens(clipped);
        if token_total + count > policy.max_tokens as usize {
            break;
        }
        token_total += count;
        spans.push(SummarySpan {
            span_id: format!("{segment_id}#{}", spans.len()),
            segment_id: segment_id.clone(),
            text: clipped.to_string(),
        });
    }
    let span_digests: Vec<String> = spans
        .iter()
        .map(|span| {
            format!(
                "{}|{}|{}",
                span.span_id,
                span.segment_id,
                Digest256::sha256(span.text.as_bytes()).to_hex()
            )
        })
        .collect();
    let transcript_hex = request.transcript_digest.to_hex();
    let admission_hex = admission.admission_digest.to_hex();
    let mut fields: Vec<&[u8]> = vec![
        request.meeting_id.as_bytes(),
        transcript_hex.as_bytes(),
        admission_hex.as_bytes(),
        admission.policy_revision.as_bytes(),
    ];
    fields.extend(span_digests.iter().map(String::as_bytes));
    let context_digest = Digest256::framed(b"medulla/meeting-summary-context/v1", &fields)?;
    let stored: Vec<StoredSpan> = spans
        .iter()
        .map(|span| {
            (
                span.span_id.clone(),
                span.segment_id.clone(),
                span.text.len() as u32,
            )
        })
        .collect();
    let node_id = context_node_id(&context_digest);
    let mut properties = BTreeMap::new();
    properties.insert("type", CONTEXT_TYPE.to_string());
    properties.insert("meeting_id", request.meeting_id.clone());
    properties.insert("transcript_digest", transcript_hex);
    properties.insert("context_digest", context_digest.to_hex());
    properties.insert("admission_digest", admission_hex);
    properties.insert("policy_revision", admission.policy_revision.clone());
    properties.insert(
        "spans_json",
        serde_json::to_string(&stored).map_err(|error| error.to_string())?,
    );
    let mut edge_properties = BTreeMap::new();
    edge_properties.insert("relationship", "contextOf".to_string());
    let methods = vec![
        Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(properties)?,
        },
        Method::AddEdge {
            source_id: node_id,
            target_id: transcript_node_id(&request.meeting_id),
            properties_msgpack: encode(edge_properties)?,
        },
    ];
    let write_set = IndexWriteSet::from_methods(
        "summary-context:",
        b"medulla/meeting-summary-context",
        methods,
    )?;
    let context = MeetingSummaryContext {
        meeting_id: request.meeting_id.clone(),
        transcript_digest: request.transcript_digest,
        citations: spans.iter().map(|span| span.span_id.clone()).collect(),
        spans,
        context_digest,
        policy_revision: admission.policy_revision.clone(),
    };
    Ok((write_set, context))
}

/// A stored `SummaryContext` row, decoded.
pub struct StoredContext {
    pub meeting_id: String,
    pub transcript_digest: Digest256,
    pub admission_digest: Digest256,
    pub spans: Vec<StoredSpan>,
}

/// Decode a stored `SummaryContext` row.
pub fn stored_context(properties: &[u8]) -> Result<StoredContext, String> {
    let map = super::decode(properties)?;
    let text = |key: &str| {
        map.get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("summary context row lacks '{key}'"))
    };
    Ok(StoredContext {
        meeting_id: text("meeting_id")?.to_string(),
        transcript_digest: Digest256::parse(text("transcript_digest")?)?,
        admission_digest: Digest256::parse(text("admission_digest")?)?,
        spans: serde_json::from_str(text("spans_json")?).map_err(|error| error.to_string())?,
    })
}
