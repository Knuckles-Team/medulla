# Seam: Served meeting and bookmark operations

Provider: medulla MEDULLA-MEDIA-VOCAB-R001.101.3
Consumer: arbora ARBORA-OPS-R065.4

## Wire shape

Arbora admits the caller, supplies the verified tenant and principal and calls `Method::MeetingTranscriptRevision`, `Method::MeetingSummaryContext`, `Method::MeetingSummaryCommit`, `Method::BookmarkSemanticSearch` and `Method::BookmarkRelations`. Medulla returns the typed result or one stable refusal code. Arbora also serves the bookmark export adapter (`ARBORA-OPS-R066`).

## Refusal codes

Meeting: `TRANSCRIPT_BASE_CONFLICT`, `TRANSCRIPT_EVENT_CONFLICT`, `TRANSCRIPT_NOT_FINAL`, `TRANSCRIPT_STALE`, `SUMMARY_LIMIT_INVALID`, `SUMMARY_LIMIT_EXCEEDED`, `SUMMARY_ADMISSION_MISSING`, `RECEIPT_UNKNOWN`, `RECEIPT_MISMATCH`, `CITATION_OUTSIDE_TEXT`. Bookmark: `BOOKMARK_POLICY_INVALID`, `BOOKMARK_LIMIT_EXCEEDED`, `BOOKMARK_INDEX_STALE`, `BOOKMARK_REVISION_MISMATCH`, `BOOKMARK_RESPONSE_TOO_LARGE`. A cross-tenant request returns `TENANT_DENIED` before any write.

## Authoritative types

The request and result types in `crates/medulla-types/src/meeting_transcript.rs` and `bookmark_query.rs`, and the method variants in `crates/medulla-protocol-types/src/protocol/method/method_11.rs`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
