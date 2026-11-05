# Seam: scion-sdk source envelopes for bookmarks and papers

Provider: scion-sdk SCION-SOURCE-INGEST-R005.8
Consumer: medulla MEDULLA-MEDIA-VOCAB-R001.102.1

## Wire shape

scion-sdk produces bookmark records and paper records with their document receipts through `SourceIngest`. Medulla accepts them on the one `Method::SourceIngest` path and adds no second ingest path. Tenant scope comes from the verified request context, never from the record body.

## Refusal codes

Paper refusals per record: `PAPER_IDENTITY_INVALID`, `PAPER_RECEIPT_MISMATCH`, `PAPER_ARTIFACT_UNAVAILABLE`, `PAPER_FIELD_PROVENANCE_MISSING`. Bookmark and index refusals: `BOOKMARK_INDEX_STALE`, `BOOKMARK_REVISION_MISMATCH`. Shared decision codes live in `MEDULLA-DECISION-ENGINE-R010.100`.

## Authoritative types

The envelope types of `crates/medulla-source-types/src/source_ingestion.rs` and the record types owned by scion-sdk. Citation edges wait for the scion-sdk rows that serve cited-paper ids (`SCION-SOURCE-INGEST-R005.31`).

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
