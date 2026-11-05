//! The meeting transcript revision function (MEDULLA-MEDIA-VOCAB-R001.101.2).

use medulla_core::meeting_transcript::{
    apply_revision, get_final, Conflict, SummaryMark, TranscriptState,
};
use medulla_types::contract::Digest256;
use medulla_types::meeting_transcript::{MeetingTranscriptRevision, TranscriptRevisionOp};

fn event(
    op: TranscriptRevisionOp,
    base: u64,
    seq: u64,
    key: &str,
    segments: &[&str],
    text: &[u8],
) -> MeetingTranscriptRevision {
    MeetingTranscriptRevision {
        meeting_id: "m-1".into(),
        op,
        base_revision: base,
        new_revision: base + 1,
        monotonic_event_sequence: seq,
        idempotency_key: key.into(),
        ordered_segment_ids: segments.iter().map(|s| s.to_string()).collect(),
        start_ms: 0,
        end_ms: 1_000,
        speaker_ref: None,
        text_digest: Digest256::sha256(text),
        provenance_ref: "capture:1".into(),
        finalization_digest: None,
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R001.101.2
#[test]
fn append_correct_finalize_and_conflicts() {
    use TranscriptRevisionOp::*;
    let empty = TranscriptState::default();
    let first = event(Append, 0, 1, "k1", &["s1"], b"hello");
    let one = apply_revision(&empty, &first).expect("append");
    assert_eq!(one.revision, 1);
    let two =
        apply_revision(&one.state, &event(Append, 1, 2, "k2", &["s2"], b"world")).expect("append");
    let three = apply_revision(
        &two.state,
        &event(DeleteSegment, 2, 3, "k3", &["s2"], b"del"),
    )
    .expect("delete");
    assert_eq!(three.state.segments.len(), 1);

    // An exact replay returns the stored revision and changes nothing.
    let replay = apply_revision(&three.state, &first).expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.revision, 1);
    assert_eq!(replay.state, three.state);

    // A base mismatch and a reused sequence with a new digest both refuse.
    let base = apply_revision(&three.state, &event(Append, 1, 9, "k9", &["s9"], b"x"));
    assert_eq!(base.unwrap_err().code(), "TRANSCRIPT_BASE_CONFLICT");
    let reused = apply_revision(&three.state, &event(Append, 3, 2, "k2", &["s3"], b"other"));
    assert_eq!(reused.unwrap_err(), Conflict::EventConflict);
    assert_eq!(Conflict::EventConflict.code(), "TRANSCRIPT_EVENT_CONFLICT");

    // Finalize seals the ordered segments.
    let sealed =
        apply_revision(&three.state, &event(Finalize, 3, 4, "k4", &[], b"fin")).expect("finalize");
    let final_one = get_final(&sealed.state).expect("finalized").clone();
    assert_eq!(final_one.ordered_segment_ids, vec!["s1".to_string()]);

    // A later Correct makes a new revision and marks the old summary stale.
    let mut with_summary = sealed.state.clone();
    with_summary.summaries.push(SummaryMark {
        summary_id: "sum-1".into(),
        transcript_digest: final_one.finalization_digest,
        stale: false,
    });
    let corrected = apply_revision(&with_summary, &event(Correct, 4, 5, "k5", &["s1"], b"hi"))
        .expect("correct");
    assert!(corrected.state.summaries.iter().all(|s| s.stale));
    assert_eq!(
        corrected.state.summaries[0].transcript_digest,
        final_one.finalization_digest
    );
    // GetFinal still names the latest sealed revision until a new seal.
    assert_eq!(get_final(&corrected.state), Some(&final_one));
    let resealed = apply_revision(&corrected.state, &event(Finalize, 5, 6, "k6", &[], b"fin2"))
        .expect("finalize");
    assert_ne!(
        get_final(&resealed.state).unwrap().finalization_digest,
        final_one.finalization_digest
    );
}
