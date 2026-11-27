//! `MeetingTranscriptRead` and `MeetingSummaryGet`: the reads of the meeting
//! transcript surface (MEDULLA-MEDIA-VOCAB-R001.101.7).
//!
//! Both are pure over the stored head state and the stored summary row the
//! graph route hands them, each already filtered by the caller's read
//! authority. An unknown meeting, an unknown summary and a row the caller
//! cannot see answer identically, so neither read is an existence oracle.

use std::collections::BTreeMap;

use medulla_core::meeting_transcript::{
    finalization_digest, get_final, FinalizedRevision, SummaryMark, TranscriptSnapshot,
};
use medulla_source_types::meeting_transcript::{
    MeetingSummaryGetAnswer, MeetingSummaryGetRequest, MeetingSummaryRecord,
    MeetingTranscriptReadAnswer, MeetingTranscriptReadRequest, SummaryClaim, TranscriptEventRecord,
    DEFAULT_TRANSCRIPT_READ_LIMIT, MAX_TRANSCRIPT_READ_LIMIT,
};
use medulla_types::contract::Digest256;

/// The node id of one committed summary.
pub fn summary_node_id(meeting_id: &str, summary_id: &str) -> String {
    format!("meeting-summary:{meeting_id}:{summary_id}")
}

/// The events of `state` after the request cursor, oldest first, with the
/// revision and seal state a subscriber needs.
pub fn read_transcript(
    state: &TranscriptSnapshot,
    request: &MeetingTranscriptReadRequest,
) -> Result<MeetingTranscriptReadAnswer, String> {
    let limit = request.limit.unwrap_or(DEFAULT_TRANSCRIPT_READ_LIMIT);
    if limit == 0 || limit > MAX_TRANSCRIPT_READ_LIMIT {
        return Err(format!(
            "TRANSCRIPT_READ_LIMIT_INVALID: limit {limit} is not in 1..={MAX_TRANSCRIPT_READ_LIMIT}"
        ));
    }
    let after = request.after_seq.unwrap_or(0);
    let mut remaining = state.events.iter().filter(|event| event.sequence > after);
    let events: Vec<TranscriptEventRecord> = remaining
        .by_ref()
        .take(limit as usize)
        .map(|event| TranscriptEventRecord {
            sequence: event.sequence,
            revision: event.revision,
            op: event.op,
            text_digest: event.digest,
            segment_ids: state
                .finalized
                .iter()
                .find(|sealed| sealed.revision == event.revision)
                .map(|sealed| sealed.ordered_segment_ids.clone())
                .unwrap_or_default(),
            finalization_digest: state
                .finalized
                .iter()
                .find(|sealed| sealed.revision == event.revision)
                .map(|sealed| sealed.finalization_digest),
        })
        .collect();
    let sealed = get_final(state);
    Ok(MeetingTranscriptReadAnswer {
        meeting_id: request.meeting_id.clone(),
        next_seq: remaining
            .next()
            .and(events.last().map(|event| event.sequence)),
        events,
        last_sequence: state.events.last().map_or(0, |event| event.sequence),
        transcript_revision: state.revision,
        context_revision: sealed.map_or(0, |sealed| sealed.revision),
        finalization_digest: sealed.map(|sealed| sealed.finalization_digest),
        segment_digests: sealed.map_or_else(BTreeMap::new, |sealed| {
            sealed_segment_digests(state, sealed)
        }),
    })
}

/// The `{segment_id: text_digest}` of the segments `sealed` covers, only when the stored
/// segments reproduce the seal's digest: a transcript that moved past the seal no longer holds
/// the sealed texts' digests, and no map is better than a map for the wrong revision.
fn sealed_segment_digests(
    state: &TranscriptSnapshot,
    sealed: &FinalizedRevision,
) -> BTreeMap<String, Digest256> {
    let ordered: Vec<(String, Digest256)> = sealed
        .ordered_segment_ids
        .iter()
        .filter_map(|id| {
            state
                .segments
                .iter()
                .find(|(segment, _)| segment == id)
                .map(|(segment, digest)| (segment.clone(), *digest))
        })
        .collect();
    if ordered.len() == sealed.ordered_segment_ids.len()
        && finalization_digest(&ordered) == sealed.finalization_digest
    {
        ordered.into_iter().collect()
    } else {
        BTreeMap::new()
    }
}

/// The summary mark a request names: the one `summary_id`, else the latest.
pub fn pick_summary<'a>(
    state: &'a TranscriptSnapshot,
    request: &MeetingSummaryGetRequest,
) -> Option<&'a SummaryMark> {
    match &request.summary_id {
        Some(summary_id) => state
            .summaries
            .iter()
            .rev()
            .find(|mark| &mark.summary_id == summary_id),
        None => state.summaries.last(),
    }
}

/// The answer of a summary get: the stored row of `mark` decoded, `None` when
/// the row is not visible to the caller.
pub fn summary_answer(
    mark: Option<&SummaryMark>,
    row: Option<&[u8]>,
) -> Result<MeetingSummaryGetAnswer, String> {
    let (Some(mark), Some(row)) = (mark, row) else {
        return Ok(MeetingSummaryGetAnswer { summary: None });
    };
    let map = super::decode(row)?;
    let text = |key: &str| {
        map.get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("meeting summary row lacks '{key}'"))
    };
    let digest = |key: &str| -> Result<Digest256, String> { Digest256::parse(text(key)?) };
    let claims: Vec<SummaryClaim> = match map.get("claims_json").and_then(serde_json::Value::as_str)
    {
        Some(json) => serde_json::from_str(json)
            .map_err(|error| format!("meeting summary claims are invalid: {error}"))?,
        None => Vec::new(),
    };
    Ok(MeetingSummaryGetAnswer {
        summary: Some(MeetingSummaryRecord {
            summary_id: mark.summary_id.clone(),
            stale: mark.stale,
            transcript_digest: mark.transcript_digest,
            context_digest: digest("context_digest")?,
            receipt_id: text("receipt_id")?.to_string(),
            receipt_digest: digest("receipt_digest")?,
            output_digest: digest("output_digest")?,
            provider: text("provider")?.to_string(),
            model: text("model")?.to_string(),
            policy_revision: text("policy_revision")?.to_string(),
            claims,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_core::meeting_transcript::{AppliedEvent, FinalizedRevision};
    use medulla_source_types::meeting_transcript::TranscriptRevisionOp;

    fn digest(byte: u8) -> Digest256 {
        Digest256::framed(b"test", &[&[byte][..]]).expect("frames")
    }

    fn state() -> TranscriptSnapshot {
        let event = |sequence: u64, revision: u64, op| AppliedEvent {
            sequence,
            idempotency_key: format!("k{sequence}"),
            digest: digest(sequence as u8),
            revision,
            op: Some(op),
        };
        TranscriptSnapshot {
            meeting_id: "m1".into(),
            revision: 3,
            segments: Vec::new(),
            events: vec![
                event(10, 1, TranscriptRevisionOp::Append),
                event(20, 2, TranscriptRevisionOp::Correct),
                event(30, 3, TranscriptRevisionOp::Finalize),
            ],
            finalized: vec![FinalizedRevision {
                revision: 3,
                ordered_segment_ids: Vec::new(),
                finalization_digest: digest(99),
            }],
            summaries: Vec::new(),
        }
    }

    fn request(after_seq: Option<u64>, limit: Option<u32>) -> MeetingTranscriptReadRequest {
        MeetingTranscriptReadRequest {
            meeting_id: "m1".into(),
            after_seq,
            limit,
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.101.7
    #[test]
    fn events_page_by_sequence_and_carry_the_seal() {
        let first = read_transcript(&state(), &request(None, Some(2))).expect("reads");
        assert_eq!(
            first.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(first.next_seq, Some(20));
        assert_eq!((first.transcript_revision, first.context_revision), (3, 3));
        let rest = read_transcript(&state(), &request(first.next_seq, None)).expect("reads");
        assert_eq!(rest.events.len(), 1);
        assert_eq!(rest.next_seq, None);
        assert_eq!(rest.events[0].op, Some(TranscriptRevisionOp::Finalize));
        assert_eq!(rest.events[0].finalization_digest, Some(digest(99)));
        assert_eq!(rest.last_sequence, 30);
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.101.7
    #[test]
    fn a_seal_names_its_segments_and_their_digests_until_the_transcript_moves_on() {
        let segments = vec![("s1".to_string(), digest(1)), ("s2".to_string(), digest(2))];
        let mut sealed_state = state();
        sealed_state.segments = segments.clone();
        sealed_state.finalized = vec![FinalizedRevision {
            revision: 3,
            ordered_segment_ids: vec!["s1".into(), "s2".into()],
            finalization_digest: finalization_digest(&segments),
        }];
        let answer = read_transcript(&sealed_state, &request(None, None)).expect("reads");
        assert!(
            answer.events[0].segment_ids.is_empty(),
            "an unsealed revision names none"
        );
        assert_eq!(
            answer.events[2].segment_ids,
            vec!["s1".to_string(), "s2".to_string()]
        );
        assert_eq!(answer.segment_digests.len(), 2);
        assert_eq!(answer.segment_digests["s1"], digest(1));
        assert_eq!(answer.segment_digests["s2"], digest(2));

        // A segment re-digested after the seal no longer reproduces it: no map is returned.
        let mut moved = sealed_state.clone();
        moved.segments[1].1 = digest(7);
        let after = read_transcript(&moved, &request(None, None)).expect("reads");
        assert!(after.segment_digests.is_empty());
        // No seal at all: none either.
        let unsealed =
            read_transcript(&TranscriptSnapshot::default(), &request(None, None)).expect("reads");
        assert!(unsealed.segment_digests.is_empty());
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.101.7
    #[test]
    fn unknown_meeting_reads_empty_and_a_bad_limit_refuses() {
        let empty =
            read_transcript(&TranscriptSnapshot::default(), &request(None, None)).expect("reads");
        assert!(empty.events.is_empty());
        assert_eq!((empty.transcript_revision, empty.last_sequence), (0, 0));
        for limit in [0, MAX_TRANSCRIPT_READ_LIMIT + 1] {
            let refused = read_transcript(&state(), &request(None, Some(limit)))
                .expect_err("limit is refused");
            assert!(refused.starts_with("TRANSCRIPT_READ_LIMIT_INVALID"));
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.101.7
    #[test]
    fn an_unseen_summary_answers_none() {
        let mut held = state();
        held.summaries.push(SummaryMark {
            summary_id: "s1".into(),
            transcript_digest: digest(99),
            stale: false,
        });
        let wanted = MeetingSummaryGetRequest {
            meeting_id: "m1".into(),
            summary_id: None,
        };
        let mark = pick_summary(&held, &wanted);
        assert_eq!(mark.map(|m| m.summary_id.as_str()), Some("s1"));
        assert_eq!(summary_answer(mark, None).expect("answers").summary, None);
        let other = MeetingSummaryGetRequest {
            meeting_id: "m1".into(),
            summary_id: Some("nope".into()),
        };
        assert!(pick_summary(&held, &other).is_none());
    }
}
