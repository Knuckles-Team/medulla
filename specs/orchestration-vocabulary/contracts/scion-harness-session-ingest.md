# Seam: harness session batch ingest

Provider: medulla MEDULLA-ORCH-VOCAB-R009.101.1
Consumer: scion SCION-SOURCE-INGEST-R013

## Wire shape

The connector sync sends `IndexHarnessSessionBatch` with a manifest (`manifest_id`, `manifest_revision` 1, `manifest_digest`, `declared_total`), a sync receipt (`sync_receipt_id`, `sync_receipt_digest`, `source_cursor`), one record per indexable session and one `UnavailableSession` per declared session whose source is missing. Medulla verifies every record against its committed `RunTrace` and writes index nodes linked to the trace. An identical batch is idempotent across restart.

## Refusal codes

`SESSION_MANIFEST_UNSUPPORTED`, `SESSION_CONTENT_CHANGED`, `SESSION_TRACE_UNKNOWN` and `SESSION_CURSOR_STALE` on the search side. A cross-tenant reference reads as absent.

## Run trace digest

`trace_digest` is the lowercase hex SHA-256 (no prefix) of the canonical JSON of the canonical submitted `RunTrace` record, the property map the connector wrote when it created the node (never properties read back from the engine), with the keys `node_type`, `tenant` and `run_revision` removed: object keys sorted at every depth, no whitespace, JSON scalars as `serde_json` writes them. Medulla computes it with `run_trace_digest` in `crates/medulla-h-ingest-repository/src/harness_session.rs`, and a connector computes the same function over the record it submitted. The record's `run_trace_revision` is the node's stored `run_revision` (a trace that stores none counts as revision 1). A record whose revision or digest differs from the stored trace is refused `SESSION_CONTENT_CHANGED`.

## Authoritative types

`HarnessSessionRecord` and `IndexHarnessSessionBatch` in `crates/medulla-types/src/harness_session.rs`.

## Test vectors

The round-trip vectors of R009.101.1 and the served tests of R009.101.1 and R009.101.2.
