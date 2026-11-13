//! Ordering of recorded replay events (MEDULLA-DECISION-ENGINE-R088.2.1).
//!
//! Events apply in sequence: each sequence number is exactly one above the
//! last applied. An event that repeats an applied sequence number with the
//! same content is idempotent and applied once; with different content it
//! is a conflict. A skipped number is a gap, and a number behind the last
//! applied one that was never applied is out of order.

use std::collections::BTreeMap;

use medulla_decision_types::decision::replay::ReplayEvent;
use medulla_decision_types::decision::statistical::body::{
    canonical_body_bytes, content_digest_of,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

use crate::decision::refusal::{Refusal, RefusalResult};

/// The events a replay applies, in order, with their digests and the cursor
/// after the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedEvents {
    /// Each applied event once, in sequence order.
    pub applied: Vec<ReplayEvent>,
    /// The content digest of each applied event, aligned with `applied`.
    pub digests: Vec<String>,
    /// `<seq>:<event digest>` of the last applied event; `None` for no events.
    pub cursor: Option<String>,
}

/// The content digest of one event: SHA-256 of its canonical encoding.
pub fn event_digest(event: &ReplayEvent) -> RefusalResult<String> {
    canonical_body_bytes(event)
        .map(|bytes| content_digest_of(&bytes))
        .map_err(|detail| Refusal::new(StatisticalErrorCode::ReplaySpecInvalid, detail))
}

/// The cursor naming `event` as the last applied one.
pub fn cursor_of(event: &ReplayEvent) -> RefusalResult<String> {
    Ok(format!("{}:{}", event.seq, event_digest(event)?))
}

/// Check `events` in the order given and return the ones to apply.
///
/// The first event starts the stream at its own sequence number. Refusals:
/// `REPLAY_SEQUENCE_GAP` (a number above the next expected one),
/// `REPLAY_OUT_OF_ORDER` (a number behind the last applied one that was not
/// applied) and `REPLAY_DUPLICATE_CONFLICT` (an applied number repeated with
/// different content).
pub fn order_events(events: &[ReplayEvent]) -> RefusalResult<OrderedEvents> {
    let mut applied: Vec<ReplayEvent> = Vec::new();
    let mut digests: Vec<String> = Vec::new();
    let mut seen: BTreeMap<u64, String> = BTreeMap::new();
    for event in events {
        let digest = event_digest(event)?;
        if let Some(earlier) = seen.get(&event.seq) {
            if *earlier == digest {
                continue;
            }
            return Err(Refusal::new(
                StatisticalErrorCode::ReplayDuplicateConflict,
                format!(
                    "event `{}` repeats sequence {} with different content",
                    event.event_id, event.seq
                ),
            ));
        }
        if let Some(last) = applied.last() {
            if event.seq < last.seq {
                return Err(Refusal::new(
                    StatisticalErrorCode::ReplayOutOfOrder,
                    format!(
                        "event `{}` has sequence {} after {}",
                        event.event_id, event.seq, last.seq
                    ),
                ));
            }
            if event.seq != last.seq + 1 {
                return Err(Refusal::new(
                    StatisticalErrorCode::ReplaySequenceGap,
                    format!(
                        "event `{}` has sequence {}, expected {}",
                        event.event_id,
                        event.seq,
                        last.seq + 1
                    ),
                ));
            }
        }
        seen.insert(event.seq, digest.clone());
        digests.push(digest);
        applied.push(event.clone());
    }
    let cursor = applied
        .last()
        .zip(digests.last())
        .map(|(event, digest)| format!("{}:{digest}", event.seq));
    Ok(OrderedEvents {
        applied,
        digests,
        cursor,
    })
}
