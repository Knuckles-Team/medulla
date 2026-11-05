# Seam: document and transcript batches from the SDK worker

Provider: medulla MEDULLA-INGEST-WORKER-R003.8
Consumer: scion-sdk SCION-SOURCE-INGEST-R010

## Wire shape

The SDK worker sends document bytes as a blob (`content_ref`) with a `media_type` payload field, and transcript batches with a `dialect` that names a `TranscriptDialect`. It never sends extracted text. The engine reads, normalizes, deduplicates and decomposes, then returns an accepted receipt or a quarantine receipt.

## Refusal codes

`DOCUMENT_NORMALIZATION_FAILED`, `ATOM_PROVENANCE_MISSING`, `MAPPING_REVISION_MISMATCH`, the four `READER_*` codes and the four `TRANSCRIPT_*` codes. A refusal changes no accepted projection.

## Authoritative types

`SourceIngestionOutcome` in `crates/medulla-source-types/src/source_document.rs` and `TranscriptThread` in `crates/medulla-source-types/src/source_transcript.rs`.

## Test vectors

`tests/ingest/test_document_normalization_served.py::test_document_batch_is_atomic` and `tests/ingest/test_transcript_served.py::test_thread_messages_are_shared_within_scope`.
