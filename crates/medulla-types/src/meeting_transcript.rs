//! Meeting transcript revisions and summary contracts
//! (MEDULLA-MEDIA-VOCAB-R001.101.1).
//!
//! Pure serde types shared by the revision function in `medulla-core` and the
//! ingest handlers of `medulla-h-ingest`. Every digest is a
//! [`Digest256`]: SHA-256, 64 lower-case hex characters on the wire.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// Fixed ceiling of `MeetingSummaryLimitPolicy::max_spans`.
pub const MAX_SUMMARY_SPANS: u32 = 128;
/// Fixed ceiling of `MeetingSummaryLimitPolicy::max_segment_bytes`.
pub const MAX_SUMMARY_SEGMENT_BYTES: u32 = 16_384;
/// Fixed ceiling of `MeetingSummaryLimitPolicy::max_tokens`.
pub const MAX_SUMMARY_TOKENS: u32 = 8_192;

/// The refusal codes of the meeting transcript surface
/// (MEDULLA-MEDIA-VOCAB-R001.101.6), `(code, meaning)`, sorted by code.
pub const REFUSALS: &[(&str, &str)] = &[
    (
        "CITATION_OUTSIDE_TEXT",
        "a summary citation lies outside every span of its context",
    ),
    (
        "RECEIPT_MISMATCH",
        "the summary receipt does not match the stored context admission or its digest",
    ),
    (
        "RECEIPT_UNKNOWN",
        "no stored summary context matches the receipt input context digest",
    ),
    (
        "SUMMARY_ADMISSION_MISSING",
        "the summary context request carries no admission reference",
    ),
    (
        "SUMMARY_LIMIT_EXCEEDED",
        "a summary limit is above its fixed ceiling",
    ),
    ("SUMMARY_LIMIT_INVALID", "a summary limit is zero"),
    ("TENANT_DENIED", "the request names another tenant"),
    (
        "TRANSCRIPT_BASE_CONFLICT",
        "the revision base does not match the current transcript revision",
    ),
    (
        "TRANSCRIPT_EVENT_CONFLICT",
        "the event reuses or lowers a sequence with a different digest",
    ),
    (
        "TRANSCRIPT_NOT_FINAL",
        "the transcript revision is not finalized",
    ),
    (
        "TRANSCRIPT_STALE",
        "the transcript digest is not the current finalized digest",
    ),
];

/// The operation of one transcript revision event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRevisionOp {
    Append,
    Correct,
    DeleteSegment,
    Finalize,
}

/// One revision event of a meeting transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingTranscriptRevision {
    pub meeting_id: String,
    pub op: TranscriptRevisionOp,
    pub base_revision: u64,
    pub new_revision: u64,
    pub monotonic_event_sequence: u64,
    pub idempotency_key: String,
    pub ordered_segment_ids: Vec<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_ref: Option<String>,
    pub text_digest: Digest256,
    pub provenance_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_digest: Option<Digest256>,
}

/// The bounds a summary context is built under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingSummaryLimitPolicy {
    pub max_spans: u32,
    pub max_segment_bytes: u32,
    pub max_tokens: u32,
}

impl MeetingSummaryLimitPolicy {
    /// Checks each limit is positive and at most its fixed ceiling.
    pub fn check(&self) -> Result<(), &'static str> {
        let limits = [
            (self.max_spans, MAX_SUMMARY_SPANS),
            (self.max_segment_bytes, MAX_SUMMARY_SEGMENT_BYTES),
            (self.max_tokens, MAX_SUMMARY_TOKENS),
        ];
        if limits.iter().any(|(value, _)| *value == 0) {
            return Err("SUMMARY_LIMIT_INVALID");
        }
        if limits.iter().any(|(value, ceiling)| value > ceiling) {
            return Err("SUMMARY_LIMIT_EXCEEDED");
        }
        Ok(())
    }
}

/// The admission a summary context was granted: the provider, model and
/// policy revision a later receipt must match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryAdmissionRef {
    pub provider: String,
    pub model: String,
    pub policy_revision: String,
    pub admission_digest: Digest256,
}

impl SummaryAdmissionRef {
    /// The admission digest over provider, model and policy revision.
    pub fn digest_of(provider: &str, model: &str, policy_revision: &str) -> Digest256 {
        Digest256::framed(
            b"medulla/meeting-summary-admission/v1",
            &[
                provider.as_bytes(),
                model.as_bytes(),
                policy_revision.as_bytes(),
            ],
        )
        .expect("three bounded fields frame")
    }
}

/// The receipt a summary producer returns with its claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryReceipt {
    pub owner_requirement_id: String,
    pub receipt_id: String,
    pub receipt_digest: Digest256,
    pub provider: String,
    pub model: String,
    pub policy_revision: String,
    pub input_context_digest: Digest256,
    pub output_digest: Digest256,
}

impl SummaryReceipt {
    /// SHA-256 over every receipt field except `receipt_digest`.
    pub fn expected_digest(&self) -> Digest256 {
        let input = self.input_context_digest.to_hex();
        let output = self.output_digest.to_hex();
        Digest256::framed(
            b"medulla/meeting-summary-receipt/v1",
            &[
                self.owner_requirement_id.as_bytes(),
                self.receipt_id.as_bytes(),
                self.provider.as_bytes(),
                self.model.as_bytes(),
                self.policy_revision.as_bytes(),
                input.as_bytes(),
                output.as_bytes(),
            ],
        )
        .expect("seven bounded fields frame")
    }
}
