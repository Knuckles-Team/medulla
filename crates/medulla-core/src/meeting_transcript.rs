//! The pure meeting transcript revision function
//! (MEDULLA-MEDIA-VOCAB-R001.101.2).
//!
//! `apply_revision(state, event)` checks the event against the current
//! transcript state and returns the next state, or a conflict. It touches no
//! store: the ingest handler commits the returned revision through the one
//! `SourceIngest` ChangeEnvelope commit.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;
use medulla_types::meeting_transcript::{MeetingTranscriptRevision, TranscriptRevisionOp};

/// The refusal a revision event meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conflict {
    /// The event's `base_revision` is not the current revision.
    BaseConflict,
    /// The event reuses or lowers a sequence with a different digest.
    EventConflict,
}

impl Conflict {
    /// The stable refusal code.
    pub fn code(self) -> &'static str {
        match self {
            Self::BaseConflict => "TRANSCRIPT_BASE_CONFLICT",
            Self::EventConflict => "TRANSCRIPT_EVENT_CONFLICT",
        }
    }
}

/// One applied event, kept so an exact replay returns the stored revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedEvent {
    pub sequence: u64,
    pub idempotency_key: String,
    pub digest: Digest256,
    pub revision: u64,
}

/// A summary written against one finalized digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryMark {
    pub summary_id: String,
    pub transcript_digest: Digest256,
    pub stale: bool,
}

/// A sealed revision of the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizedRevision {
    pub revision: u64,
    pub ordered_segment_ids: Vec<String>,
    pub finalization_digest: Digest256,
}

/// The transcript state one event is applied to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptState {
    pub meeting_id: String,
    pub revision: u64,
    /// Ordered `(segment_id, text_digest)` pairs.
    pub segments: Vec<(String, Digest256)>,
    pub events: Vec<AppliedEvent>,
    pub finalized: Vec<FinalizedRevision>,
    pub summaries: Vec<SummaryMark>,
}

/// The result of one applied event: the next state and whether the event was
/// an exact replay of a stored one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub state: TranscriptState,
    pub revision: u64,
    pub replayed: bool,
}

/// The finalization digest over the ordered segment digests.
pub fn finalization_digest(segments: &[(String, Digest256)]) -> Digest256 {
    let fields: Vec<&[u8]> = segments
        .iter()
        .map(|(_, d)| d.as_bytes().as_slice())
        .collect();
    Digest256::framed(b"medulla/meeting-transcript-final/v1", &fields)
        .expect("segment digests frame")
}

/// Applies one revision event to `state`.
pub fn apply_revision(
    state: &TranscriptState,
    event: &MeetingTranscriptRevision,
) -> Result<Revision, Conflict> {
    if let Some(stored) = state
        .events
        .iter()
        .find(|e| e.idempotency_key == event.idempotency_key && e.digest == event.text_digest)
    {
        return Ok(Revision {
            state: state.clone(),
            revision: stored.revision,
            replayed: true,
        });
    }
    let last_sequence = state.events.last().map_or(0, |e| e.sequence);
    if !state.events.is_empty() && event.monotonic_event_sequence <= last_sequence {
        return Err(Conflict::EventConflict);
    }
    if event.base_revision != state.revision {
        return Err(Conflict::BaseConflict);
    }
    let mut next = state.clone();
    if next.meeting_id.is_empty() {
        next.meeting_id = event.meeting_id.clone();
    }
    let revision = state.revision + 1;
    match event.op {
        TranscriptRevisionOp::Append => {
            for id in &event.ordered_segment_ids {
                next.segments.push((id.clone(), event.text_digest));
            }
        }
        TranscriptRevisionOp::Correct => {
            for id in &event.ordered_segment_ids {
                match next.segments.iter_mut().find(|(sid, _)| sid == id) {
                    Some(segment) => segment.1 = event.text_digest,
                    None => next.segments.push((id.clone(), event.text_digest)),
                }
            }
        }
        TranscriptRevisionOp::DeleteSegment => {
            next.segments
                .retain(|(sid, _)| !event.ordered_segment_ids.contains(sid));
        }
        TranscriptRevisionOp::Finalize => {
            next.finalized.push(FinalizedRevision {
                revision,
                ordered_segment_ids: next.segments.iter().map(|(id, _)| id.clone()).collect(),
                finalization_digest: finalization_digest(&next.segments),
            });
        }
    }
    if event.op != TranscriptRevisionOp::Finalize {
        // A change after a seal makes every summary of an older digest stale;
        // no summary text is rewritten.
        for summary in &mut next.summaries {
            summary.stale = true;
        }
    }
    next.revision = revision;
    next.events.push(AppliedEvent {
        sequence: event.monotonic_event_sequence,
        idempotency_key: event.idempotency_key.clone(),
        digest: event.text_digest,
        revision,
    });
    Ok(Revision {
        state: next,
        revision,
        replayed: false,
    })
}

/// `GetFinal`: the latest finalized revision, when one exists.
pub fn get_final(state: &TranscriptState) -> Option<&FinalizedRevision> {
    state.finalized.last()
}
