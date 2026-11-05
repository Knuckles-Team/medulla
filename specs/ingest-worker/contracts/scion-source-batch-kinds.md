# Seam: source batch kinds from connectors

Provider: medulla MEDULLA-INGEST-WORKER-R902.3.1
Consumer: scion-sdk SCION-SOURCE-INGEST-R805.1

## Wire shape

A connector sends `SourceIngest` with `kind` one of records, access, groups, enumeration, containers, proposals, facts or transcripts. A non-record kind carries one matching `extension` and empty `records`, `relationships`, `withdrawals` and `authoritative_live_ids`. Permission sync (R805.1, R805.4), identifier enumeration (R805.2), container trees (R805.3), the `_governance` member (R900, R901) and row versions travel in these batches. Stream chaining (R004.20) uses `previous_digest` of the stream head.

## Refusal codes

`SOURCE_BATCH_KIND_MISMATCH`, `SOURCE_RECORD_UNKNOWN`, `SOURCE_GROUP_TOO_LARGE`, `SOURCE_MASS_DELETE_GUARD`, `SOURCE_CONTAINER_CYCLE`, `SOURCE_CONTAINER_TOO_DEEP`, `ROW_VERSION_INVALID` and `STALE_CONTENT_VERSION`.

## Authoritative types

`SourceBatchKind` and `SourceBatchExtension` in `crates/medulla-source-types/src/source_ingestion.rs`.

## Test vectors

The Rust round-trip vectors beside the batch type and `tests/ingest/test_source_batch_kind_served.py::test_mismatched_kind_is_refused`.
