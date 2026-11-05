//! `MeetingSummaryCommit`: a produced summary admitted against its stored
//! context (MEDULLA-MEDIA-VOCAB-R001.101.5).
//!
//! The receipt must name a stored `SummaryContext`, match that context's
//! admission (provider, model, policy revision) and carry a `receipt_digest`
//! equal to SHA-256 over its other fields; every citation must lie inside a
//! span of that context, and the transcript must still be at the digest the
//! context was cut from. Only then do the `MeetingSummary` node, its claim
//! nodes and their `cites` edges commit, with the head's summary mark, as one
//! write-set.

use std::collections::BTreeMap;

use medulla_core::meeting_transcript::{SummaryMark, TranscriptState};
use medulla_protocol_types::protocol::Method;
use medulla_types::meeting_transcript::{
    MeetingSummaryCommitReceipt, MeetingSummaryCommitRequest, SummaryAdmissionRef,
};

use super::context::{context_node_id, current_final, StoredContext};
use super::{encode, head_method, segment_node_id};
use crate::repository_index::{stored_value, IndexWriteSet};

/// Node type of one committed summary.
pub const SUMMARY_TYPE: &str = "MeetingSummary";
/// Node type of one summary claim.
pub const CLAIM_TYPE: &str = "SummaryClaim";
/// Evidence class of every committed summary (MEDULLA-MEDIA-VOCAB-R001.101.6).
pub const SUMMARY_EVIDENCE_CLASS: &str = "Observation";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn edge(source: &str, target: &str, relationship: &str) -> Result<Method, String> {
    let mut properties = BTreeMap::new();
    properties.insert("relationship", relationship.to_string());
    Ok(Method::AddEdge {
        source_id: source.into(),
        target_id: target.into(),
        properties_msgpack: encode(properties)?,
    })
}

/// Check the receipt and citations against `context` (the stored row the
/// receipt's `input_context_digest` names, `None` when absent) and lower the
/// summary. Every refusal returns before a write-set exists.
pub fn commit_summary(
    state: &TranscriptState,
    context: Option<&StoredContext>,
    request: &MeetingSummaryCommitRequest,
) -> Result<(IndexWriteSet, MeetingSummaryCommitReceipt), String> {
    let receipt = &request.receipt;
    let Some(context) = context.filter(|context| context.meeting_id == request.meeting_id) else {
        return Err(refuse(
            "RECEIPT_UNKNOWN",
            format!(
                "no summary context {}",
                context_node_id(&receipt.input_context_digest)
            ),
        ));
    };
    let admission =
        SummaryAdmissionRef::digest_of(&receipt.provider, &receipt.model, &receipt.policy_revision);
    if admission != context.admission_digest {
        return Err(refuse(
            "RECEIPT_MISMATCH",
            "provider, model or policy revision differs from the context admission",
        ));
    }
    if receipt.receipt_digest != receipt.expected_digest() {
        return Err(refuse(
            "RECEIPT_MISMATCH",
            "receipt_digest is not SHA-256 over the other receipt fields",
        ));
    }
    let spans: BTreeMap<&str, (&str, u32)> = context
        .spans
        .iter()
        .map(|(span, segment, bytes)| (span.as_str(), (segment.as_str(), *bytes)))
        .collect();
    let mut citation_count = 0u32;
    for claim in &request.claims {
        for citation in &claim.citations {
            let inside = spans
                .get(citation.span_id.as_str())
                .is_some_and(|(_, bytes)| citation.start < citation.end && citation.end <= *bytes);
            if !inside {
                return Err(refuse(
                    "CITATION_OUTSIDE_TEXT",
                    format!(
                        "claim '{}' cites outside span '{}'",
                        claim.claim_id, citation.span_id
                    ),
                ));
            }
            citation_count += 1;
        }
    }
    current_final(state, &context.transcript_digest)?;

    let summary_id = format!(
        "meeting-summary:{}:{}",
        request.meeting_id, request.summary_id
    );
    let context_id = context_node_id(&receipt.input_context_digest);
    let mut next = state.clone();
    next.summaries.push(SummaryMark {
        summary_id: request.summary_id.clone(),
        transcript_digest: context.transcript_digest,
        stale: false,
    });
    let mut methods = vec![head_method(&next)?];
    let mut properties = BTreeMap::new();
    properties.insert("type", SUMMARY_TYPE.to_string());
    properties.insert("meeting_id", request.meeting_id.clone());
    properties.insert("summary_id", stored_value(&request.summary_id).to_string());
    properties.insert("evidence_class", SUMMARY_EVIDENCE_CLASS.to_string());
    properties.insert("transcript_digest", context.transcript_digest.to_hex());
    properties.insert("context_digest", receipt.input_context_digest.to_hex());
    properties.insert("receipt_id", stored_value(&receipt.receipt_id).to_string());
    properties.insert("receipt_digest", receipt.receipt_digest.to_hex());
    properties.insert(
        "owner_requirement_id",
        stored_value(&receipt.owner_requirement_id).to_string(),
    );
    properties.insert("provider", stored_value(&receipt.provider).to_string());
    properties.insert("model", stored_value(&receipt.model).to_string());
    properties.insert("policy_revision", receipt.policy_revision.clone());
    properties.insert("output_digest", receipt.output_digest.to_hex());
    methods.push(Method::AddNode {
        node_id: summary_id.clone(),
        properties_msgpack: encode(properties)?,
    });
    methods.push(edge(&summary_id, &context_id, "summarizes")?);
    for claim in &request.claims {
        let claim_id = format!("{summary_id}:claim:{}", claim.claim_id);
        let mut properties = BTreeMap::new();
        properties.insert("type", CLAIM_TYPE.to_string());
        properties.insert("text", stored_value(&claim.text).to_string());
        properties.insert("summary_id", request.summary_id.clone());
        methods.push(Method::AddNode {
            node_id: claim_id.clone(),
            properties_msgpack: encode(properties)?,
        });
        methods.push(edge(&claim_id, &summary_id, "claimOf")?);
        let mut cited = std::collections::BTreeSet::new();
        for citation in &claim.citations {
            let (segment, _) = spans[citation.span_id.as_str()];
            if cited.insert(segment) {
                methods.push(edge(
                    &claim_id,
                    &segment_node_id(&request.meeting_id, segment),
                    "cites",
                )?);
            }
        }
    }
    let write_set = IndexWriteSet::from_methods(
        "meeting-summary:",
        b"medulla/meeting-summary-commit",
        methods,
    )?;
    Ok((
        write_set,
        MeetingSummaryCommitReceipt {
            summary_id: request.summary_id.clone(),
            claim_count: request.claims.len() as u32,
            citation_count,
            transcript_digest: context.transcript_digest,
        },
    ))
}
