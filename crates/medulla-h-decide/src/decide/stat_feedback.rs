//! `DecisionLog.learn` with a feedback event (MEDULLA-DECISION-ENGINE-R100.1.1).
//!
//! A caller reports how a committed retrieval run turned out. The event is
//! complete only when the record is visible, the committer matches
//! `check_joinable`, the executed option is the event's option, the record
//! digest matches, a retrieval outcome is stored for the record and every
//! evidence id was returned by that outcome. The event is stored once under
//! `feedback/<event id>/<source revision>`: an equal replay answers the stored
//! row, the same key with other content is an `IDEMPOTENCY_CONFLICT` and an
//! event older than a stored revision of its id is refused. Every check runs
//! before the write, so a refusal writes nothing.

use medulla_decision_types::decision::statistical::feedback::{
    feedback_key, feedback_prefix, FeedbackEvent, StoredFeedback, FEEDBACK_EVIDENCE_MISMATCH,
    FEEDBACK_JOIN_INCOMPLETE, FEEDBACK_SOURCE_STALE, KNOBS,
};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::DecisionPolicy;

use super::stat_executor::ExecutionContext;
use super::stat_log::{executed_option, store_once, visible_entry, LogReader};
use super::stat_retrieval::{check_joinable, stored_outcome};
use super::stat_support::invalid_parameter as invalid;

fn coded(code: RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The highest source revision already stored for `event_id`.
fn highest_stored_revision(ctx: &ExecutionContext, event_id: &str) -> Result<u64, String> {
    Ok(ctx
        .store
        .decision_artifacts_with_prefix(ctx.tenant_id, &feedback_prefix(event_id), 100_000)?
        .iter()
        .filter_map(|(key, _)| key.rsplit('/').next()?.parse::<u64>().ok())
        .max()
        .unwrap_or(0))
}

/// Join one feedback event to its committed retrieval-plan record and store it.
pub(super) fn record_feedback(
    ctx: &ExecutionContext,
    reader: &LogReader,
    event: FeedbackEvent,
) -> Result<StoredFeedback, String> {
    let max_evidence = match DecisionPolicy::engine_default().knob(&KNOBS[0]) {
        KnobValue::Integer(max) => usize::try_from(max).unwrap_or(0),
        _ => 0,
    };
    event.check(max_evidence).map_err(invalid)?;

    let entry = visible_entry(ctx.store, reader, &event.record_id)?.ok_or_else(|| {
        coded(
            FEEDBACK_JOIN_INCOMPLETE,
            format!("no committed record {} is visible", event.record_id),
        )
    })?;
    // The sealed record digest is written `sha256:<hex>`; the event carries the hex.
    let digest = entry
        .record
        .record_digest
        .strip_prefix("sha256:")
        .unwrap_or(&entry.record.record_digest)
        .to_string();
    let executed = executed_option(&entry.record.outcome).map(str::to_string);
    check_joinable(Some(entry), reader)?;
    if executed.as_deref() != Some(event.option_id.as_str()) {
        return Err(coded(
            FEEDBACK_JOIN_INCOMPLETE,
            format!(
                "record {} did not execute option {}",
                event.record_id, event.option_id
            ),
        ));
    }
    if digest != event.record_digest {
        return Err(coded(
            FEEDBACK_EVIDENCE_MISMATCH,
            format!("the digest of record {} differs", event.record_id),
        ));
    }
    let outcome = stored_outcome(ctx.store, ctx.tenant_id, &event.record_id)?.ok_or_else(|| {
        coded(
            FEEDBACK_JOIN_INCOMPLETE,
            format!("record {} has no stored outcome", event.record_id),
        )
    })?;
    if let Some(unknown) = event.evidence_ids.iter().find(|id| {
        !outcome
            .outcome
            .returned
            .iter()
            .any(|returned| &returned.evidence_id == *id)
    }) {
        return Err(coded(
            FEEDBACK_EVIDENCE_MISMATCH,
            format!("evidence {unknown} was not returned by record {}", event.record_id),
        ));
    }
    let stored_revision = highest_stored_revision(ctx, &event.event_id)?;
    if stored_revision > event.source_revision {
        return Err(coded(
            FEEDBACK_SOURCE_STALE,
            format!(
                "event {} is stored at revision {stored_revision}, above {}",
                event.event_id, event.source_revision
            ),
        ));
    }
    let premises = vec![
        format!("feedback:event_id={}", event.event_id),
        format!("feedback:source_revision={}", event.source_revision),
        format!("feedback:record_digest={}", event.record_digest),
    ];
    let key = feedback_key(&event.event_id, event.source_revision);
    let stored = StoredFeedback {
        event,
        producer: reader.principal.clone(),
        recorded_at_ms: ctx.now_ms,
        premises,
    };
    store_once(ctx, key, "feedback event", stored, |existing, fresh| {
        existing.event == fresh.event && existing.producer == fresh.producer
    })
}
