//! Round-trip contract of the meeting transcript types
//! (MEDULLA-MEDIA-VOCAB-R001.101.1).

use medulla_types::contract::Digest256;
use medulla_types::meeting_transcript::{
    MeetingSummaryLimitPolicy, MeetingTranscriptRevision, SummaryAdmissionRef, SummaryReceipt,
    TranscriptRevisionOp, REFUSALS,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value);
}

fn rejects_unknown_field<T: Serialize + DeserializeOwned>(value: &T) {
    let mut json = serde_json::to_value(value).expect("serializes");
    json.as_object_mut()
        .expect("an object")
        .insert("unexpected".into(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<T>(json).is_err());
}

// spec: MEDULLA-MEDIA-VOCAB-R001.101.1
#[test]
fn records_round_trip() {
    let digest = Digest256::sha256(b"segment text");
    for op in [
        TranscriptRevisionOp::Append,
        TranscriptRevisionOp::Correct,
        TranscriptRevisionOp::DeleteSegment,
        TranscriptRevisionOp::Finalize,
    ] {
        round_trip(&op);
    }
    let revision = MeetingTranscriptRevision {
        meeting_id: "m-1".into(),
        op: TranscriptRevisionOp::Append,
        base_revision: 0,
        new_revision: 1,
        monotonic_event_sequence: 1,
        idempotency_key: "k-1".into(),
        ordered_segment_ids: vec!["s-1".into()],
        start_ms: 0,
        end_ms: 1_000,
        speaker_ref: Some("speaker:redacted".into()),
        text_digest: digest,
        provenance_ref: "capture:1".into(),
        finalization_digest: None,
    };
    round_trip(&revision);
    rejects_unknown_field(&revision);
    assert_eq!(
        serde_json::to_value(&revision).unwrap()["text_digest"],
        serde_json::Value::String(digest.to_hex())
    );
    assert_eq!(digest.to_hex().len(), 64);

    let policy = MeetingSummaryLimitPolicy {
        max_spans: 16,
        max_segment_bytes: 4_096,
        max_tokens: 2_048,
    };
    round_trip(&policy);
    rejects_unknown_field(&policy);
    assert!(policy.check().is_ok());

    let admission = SummaryAdmissionRef {
        provider: "local".into(),
        model: "m".into(),
        policy_revision: "p1".into(),
        admission_digest: SummaryAdmissionRef::digest_of("local", "m", "p1"),
    };
    round_trip(&admission);
    rejects_unknown_field(&admission);

    let mut receipt = SummaryReceipt {
        owner_requirement_id: "MEDULLA-MEDIA-VOCAB-R001.101.5".into(),
        receipt_id: "r-1".into(),
        receipt_digest: digest,
        provider: "local".into(),
        model: "m".into(),
        policy_revision: "p1".into(),
        input_context_digest: digest,
        output_digest: digest,
    };
    receipt.receipt_digest = receipt.expected_digest();
    round_trip(&receipt);
    rejects_unknown_field(&receipt);

    let codes: Vec<&str> = REFUSALS.iter().map(|(code, _)| code.as_str()).collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(codes, sorted, "codes are sorted and unique");
}
