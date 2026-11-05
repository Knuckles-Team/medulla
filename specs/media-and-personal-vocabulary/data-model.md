# MEDULLA-MEDIA-VOCAB — Data model

This file describes the ontology classes and the Rust types that the rows of this spec add or reuse.

## Entities

### Ontology classes

- Media works, libraries, files, playback sessions, broadcast streams, download requests, transfers, grab events and indexers: `crates/medulla-core/ontology/media-v1.ttl`, rows `MEDULLA-MEDIA-VOCAB-R001.1` to `R001.4`.
- Feeds, articles, research papers, searches, social posts, campaigns, subscribers and chat: `content-v1.ttl` and `media-v1.ttl`, rows `R002.1` to `R002.4`.
- Transcript segments reuse `:Transcript` and `:TranscriptSegment` of `personal-v1.ttl`; media-v1 adds only their properties.
- Identity keys are datatype properties scoped by class and are never inverse-functional.

### MediaTag

- Fields: tagged media, taxonomy id and version, kind (`object` or `context`), confidence in [0, 1], evidence reference, producer id and revision, media revision and provenance digest.
- Owner: `media-v1.ttl`, row `MEDULLA-MEDIA-VOCAB-R001.100.1`.
- Relation: one `:taggedMedia` edge to the media item; the node carries the item's policy version.

### Meeting transcript types

- New file: `crates/medulla-types/src/meeting_transcript.rs`, row `MEDULLA-MEDIA-VOCAB-R001.101.1`.
- `TranscriptRevisionOp`: append, correct, delete segment, finalize. `MeetingTranscriptRevision` holds the meeting id, base and new revision, a monotonic event sequence, an idempotency key, ordered segment ids and digests.
- `MeetingSummaryLimitPolicy`, `SummaryAdmissionRef` and `SummaryReceipt` bound the summary context and commit. Digests are SHA-256 as 64 lowercase hex characters.

### Bookmark query types

- New file: `crates/medulla-types/src/bookmark_query.rs`, row `MEDULLA-MEDIA-VOCAB-R001.102.1`.
- `BookmarkSemanticSearchRequest`, `BookmarkRelationsRequest`, `BookmarkQueryLimitPolicy`, `BookmarkHit`, `BookmarkEdge` and `BookmarkBinding`; the binding pins source, index, snapshot and authorization revisions.

### MediaAttachment and MediaDerivation

- `MediaAttachment{blob_digest, mime, byte_len, derivation_ids}` in `crates/medulla-work-types/src/memory.rs`, row `MEDULLA-MEDIA-VOCAB-R650.1`; the node class `:MediaDerivation` joins media-v1.ttl.
- Relation: a `MemoryRecord` cites the blob through its attachments; derivations are separate nodes linked by `:derivedFrom`.

### Refusal tables

- Each new family declares a `REFUSALS` const (`meeting_transcript.rs`, `bookmark_query.rs`, `media_tag.rs`, `paper_ingest.rs` in `crates/medulla-types/src/`, and the `memory` family in `memory.rs`), collected by the uniqueness test of `MEDULLA-DECISION-ENGINE-R010.100.1`.
