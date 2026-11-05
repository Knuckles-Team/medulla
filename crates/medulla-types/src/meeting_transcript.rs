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
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRevisionOp {
    Append,
    Correct,
    DeleteSegment,
    Finalize,
}

/// One revision event of a meeting transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
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
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub text_digest: Digest256,
    pub provenance_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "contract-schema", schemars(with = "Option<String>"))]
    pub finalization_digest: Option<Digest256>,
}

/// The bounds a summary context is built under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummaryAdmissionRef {
    pub provider: String,
    pub model: String,
    pub policy_revision: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
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
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummaryReceipt {
    pub owner_requirement_id: String,
    pub receipt_id: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub receipt_digest: Digest256,
    pub provider: String,
    pub model: String,
    pub policy_revision: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub input_context_digest: Digest256,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
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

/// The answer of one committed (or replayed) transcript revision event
/// (MEDULLA-MEDIA-VOCAB-R001.101.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MeetingTranscriptRevisionReceipt {
    pub meeting_id: String,
    pub revision: u64,
    pub replayed: bool,
    /// The sealed digest when the event was a `finalize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "contract-schema", schemars(with = "Option<String>"))]
    pub finalization_digest: Option<Digest256>,
}

/// One segment text offered for a summary context; its SHA-256 must equal the
/// segment's stored `text_digest`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummarySegmentText {
    pub segment_id: String,
    pub text: String,
}

/// `MeetingSummaryContext` request (MEDULLA-MEDIA-VOCAB-R001.101.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MeetingSummaryContextRequest {
    pub meeting_id: String,
    /// The finalized transcript digest the context is built from.
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub transcript_digest: Digest256,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<SummaryAdmissionRef>,
    pub policy: MeetingSummaryLimitPolicy,
    /// The texts of the finalized segments, in any order.
    pub segments: Vec<SummarySegmentText>,
}

/// One bounded span of a summary context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummarySpan {
    pub span_id: String,
    pub segment_id: String,
    pub text: String,
}

/// The bounded context a summary is produced from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MeetingSummaryContext {
    pub meeting_id: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub transcript_digest: Digest256,
    pub spans: Vec<SummarySpan>,
    /// The span ids a claim may cite, in span order.
    pub citations: Vec<String>,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub context_digest: Digest256,
    pub policy_revision: String,
}

/// A claim citation: a byte range `[start, end)` inside one context span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummaryCitation {
    pub span_id: String,
    pub start: u32,
    pub end: u32,
}

/// One summary claim with its citations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SummaryClaim {
    pub claim_id: String,
    pub text: String,
    pub citations: Vec<SummaryCitation>,
}

/// `MeetingSummaryCommit` request (MEDULLA-MEDIA-VOCAB-R001.101.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MeetingSummaryCommitRequest {
    pub meeting_id: String,
    pub summary_id: String,
    pub claims: Vec<SummaryClaim>,
    pub receipt: SummaryReceipt,
}

/// The answer of one committed summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MeetingSummaryCommitReceipt {
    pub summary_id: String,
    pub claim_count: u32,
    pub citation_count: u32,
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub transcript_digest: Digest256,
}
