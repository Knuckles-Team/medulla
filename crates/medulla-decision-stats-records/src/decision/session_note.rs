//! Session-note rules (MEDULLA-DECISION-ENGINE-R150.2).
//!
//! [`annotate_notes`] is a pure, deterministic function of the stored notes,
//! the task vector, the clock and the current source revision. It flags each
//! note stale, archived or in conflict and says whether it is applied; it
//! hides nothing and overwrites nothing. The caller reads the stored embedding
//! of each note through `GraphVectors`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_statistical_types::decision::DecisionPolicy;
use medulla_types::contract::Digest256;
use medulla_work_types::memory::{NoteView, SessionNoteBody};
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

const DAY_MS: u64 = 86_400_000;

/// A note is stale after this many days.
pub const NOTE_STALE_AFTER: KnobSpec = KnobSpec {
    name: "memory.note_stale_after",
    unit: "days",
    default: KnobValue::Integer(7),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(90),
    safety: KnobSafety::Tunable,
};

/// A note is archived after this many days.
pub const NOTE_ARCHIVE_AFTER: KnobSpec = KnobSpec {
    name: "memory.note_archive_after",
    unit: "days",
    default: KnobValue::Integer(14),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(365),
    safety: KnobSafety::Tunable,
};

/// The relevance a note must reach to be applied.
pub const NOTE_MIN_RELEVANCE: KnobSpec = KnobSpec {
    name: "memory.note_min_relevance",
    unit: "ratio",
    default: KnobValue::DecimalMicros(700_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// The session-note knobs.
pub const KNOBS: &[KnobSpec] = &[NOTE_STALE_AFTER, NOTE_ARCHIVE_AFTER, NOTE_MIN_RELEVANCE];

/// The resolved values of the session-note knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteKnobs {
    pub stale_after_ms: u64,
    pub archive_after_ms: u64,
    /// The relevance floor in `[0, 10000]`.
    pub min_relevance_q4: u16,
}

impl NoteKnobs {
    /// The values `policy` resolves, each clamped to its knob range.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let integer = |spec: &KnobSpec, low: i64, high: i64| match policy.knob(spec) {
            KnobValue::Integer(value) | KnobValue::DecimalMicros(value) => value.clamp(low, high),
            KnobValue::DurationMs(value) => i64::try_from(value).unwrap_or(high).clamp(low, high),
        };
        let days = |spec: &KnobSpec, high: i64| {
            u64::try_from(integer(spec, 1, high)).unwrap_or(1) * DAY_MS
        };
        let micros = integer(&NOTE_MIN_RELEVANCE, 0, 1_000_000);
        Self {
            stale_after_ms: days(&NOTE_STALE_AFTER, 90),
            archive_after_ms: days(&NOTE_ARCHIVE_AFTER, 365),
            min_relevance_q4: u16::try_from(micros / 100).unwrap_or(10_000),
        }
    }
}

impl Default for NoteKnobs {
    fn default() -> Self {
        Self::from_policy(&DecisionPolicy::engine_default())
    }
}

/// One stored note with the embedding it is recalled by.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredNote {
    pub body: SessionNoteBody,
    pub revision: u64,
    pub embedding: Vec<f32>,
    pub created_ms: u64,
}

/// Cosine similarity of two vectors, clamped to `[0, 1]` and rounded half to
/// even to four decimals, as an integer in `[0, 10000]`. A vector of another
/// width, or of zero length, has no relevance.
pub fn relevance_q4(note: &[f32], task: &[f32]) -> u16 {
    if note.is_empty() || note.len() != task.len() {
        return 0;
    }
    let (mut dot, mut norm_note, mut norm_task) = (0.0f64, 0.0f64, 0.0f64);
    for (a, b) in note.iter().zip(task) {
        let (a, b) = (f64::from(*a), f64::from(*b));
        dot += a * b;
        norm_note += a * a;
        norm_task += b * b;
    }
    if norm_note == 0.0 || norm_task == 0.0 || !dot.is_finite() {
        return 0;
    }
    let cosine = (dot / (norm_note.sqrt() * norm_task.sqrt())).clamp(0.0, 1.0);
    (cosine * 10_000.0).round_ties_even() as u16
}

/// Flag every note under the default knobs. See [`annotate_notes_with`].
pub fn annotate_notes(
    notes: &[StoredNote],
    task_vector: &[f32],
    now_ms: u64,
    current_revision: u64,
) -> Vec<NoteView> {
    annotate_notes_with(
        notes,
        task_vector,
        now_ms,
        current_revision,
        &NoteKnobs::default(),
    )
}

/// Flag every note, in input order.
///
/// A note is stale when its age exceeds the stale knob or its `source_revision`
/// differs from `current_revision`; it is archived when its age exceeds the
/// archive knob. Two or more unarchived notes with one `input_digest` and
/// different `output_digest` values are all in conflict. A note is applied when
/// it is neither archived nor in conflict and its relevance reaches the floor.
pub fn annotate_notes_with(
    notes: &[StoredNote],
    task_vector: &[f32],
    now_ms: u64,
    current_revision: u64,
    knobs: &NoteKnobs,
) -> Vec<NoteView> {
    let age = |note: &StoredNote| now_ms.saturating_sub(note.created_ms);
    let archived = |note: &StoredNote| age(note) > knobs.archive_after_ms;
    let mut outputs: BTreeMap<&Digest256, BTreeSet<&Digest256>> = BTreeMap::new();
    for note in notes.iter().filter(|note| !archived(note)) {
        outputs
            .entry(&note.body.input_digest)
            .or_default()
            .insert(&note.body.output_digest);
    }
    notes
        .iter()
        .map(|note| {
            let archived = archived(note);
            let conflict = !archived
                && outputs
                    .get(&note.body.input_digest)
                    .is_some_and(|distinct| distinct.len() > 1);
            let relevance_q4 = relevance_q4(&note.embedding, task_vector);
            NoteView {
                note_id: note.body.note_id.clone(),
                revision: note.revision,
                relevance_q4,
                stale: age(note) > knobs.stale_after_ms
                    || note.body.source_revision != current_revision,
                archived,
                conflict,
                applied: !archived && !conflict && relevance_q4 >= knobs.min_relevance_q4,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 100 * DAY_MS;

    fn digest(byte: u8) -> Digest256 {
        Digest256::from_bytes([byte; 32])
    }

    fn note(id: &str, input: u8, output: u8, age_days: u64, embedding: Vec<f32>) -> StoredNote {
        StoredNote {
            body: SessionNoteBody {
                schema_version: 1,
                note_id: id.to_string(),
                run_id: "run".into(),
                session_id: "session".into(),
                task_id: "task".into(),
                user_id: "user".into(),
                input_digest: digest(input),
                output_digest: digest(output),
                source_revision: 5,
                content: "c".into(),
            },
            revision: 1,
            embedding,
            created_ms: NOW - age_days * DAY_MS,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R150.2
    #[test]
    fn capture_and_apply() {
        // Relevance rounds half to even to four decimals and clamps below zero.
        assert_eq!(relevance_q4(&[1.0, 0.0], &[1.0, 0.0]), 10_000);
        assert_eq!(relevance_q4(&[1.0, 0.0], &[-1.0, 0.0]), 0);
        assert_eq!(relevance_q4(&[1.0, 0.0], &[0.0, 1.0]), 0);
        assert_eq!(relevance_q4(&[1.0], &[1.0, 0.0]), 0);
        assert_eq!(relevance_q4(&[3.0, 4.0], &[1.0, 0.0]), 6_000);
        assert_eq!(relevance_q4(&[1.0, 1.0], &[1.0, 0.0]), 7_071);

        let task = [1.0f32, 0.0];
        let near = vec![1.0f32, 0.0];
        let notes = vec![
            note("fresh", 1, 1, 0, near.clone()),
            // Exactly seven days old is not stale; one millisecond more is.
            note("day7", 2, 2, 7, near.clone()),
            note("day8", 3, 3, 8, near.clone()),
            // Exactly fourteen days old is not archived; fifteen is.
            note("day14", 4, 4, 14, near.clone()),
            note("day15", 5, 5, 15, near.clone()),
            note("irrelevant", 6, 6, 0, vec![0.0, 1.0]),
        ];
        let views = annotate_notes(&notes, &task, NOW, 5);
        let by_id = |id: &str| views.iter().find(|view| view.note_id == id).unwrap();
        assert!(!by_id("fresh").stale && by_id("fresh").applied);
        assert!(!by_id("day7").stale);
        assert!(by_id("day8").stale && !by_id("day8").archived && by_id("day8").applied);
        assert!(!by_id("day14").archived);
        assert!(by_id("day15").archived && !by_id("day15").applied);
        assert!(!by_id("irrelevant").applied);
        assert_eq!(by_id("irrelevant").relevance_q4, 0);
        let stale_by_revision = annotate_notes(&notes[..1], &task, NOW, 6);
        assert!(stale_by_revision[0].stale);

        // Conflict: one input, two outputs among unarchived notes; every note
        // stays visible and none is applied. An archived note is outside it.
        let conflicting = vec![
            note("a", 9, 1, 0, near.clone()),
            note("b", 9, 2, 1, near.clone()),
            note("old", 9, 3, 15, near.clone()),
            note("same-1", 8, 4, 0, near.clone()),
            note("same-2", 8, 4, 0, near),
        ];
        let views = annotate_notes(&conflicting, &task, NOW, 5);
        let by_id = |id: &str| views.iter().find(|view| view.note_id == id).unwrap();
        assert!(by_id("a").conflict && !by_id("a").applied);
        assert!(by_id("b").conflict && !by_id("b").applied);
        assert!(!by_id("old").conflict && by_id("old").archived);
        assert!(!by_id("same-1").conflict && by_id("same-1").applied);
        assert_eq!(views.len(), conflicting.len());
        assert_eq!(annotate_notes(&conflicting, &task, NOW, 5), views);
    }
}
