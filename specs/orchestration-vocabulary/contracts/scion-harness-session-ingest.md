# Seam: harness session batch ingest

Provider: medulla MEDULLA-ORCH-VOCAB-R009.101.1
Consumer: scion SCION-SOURCE-INGEST-R013

## Wire shape

The connector sync sends `IndexHarnessSessionBatch` with a manifest, a sync receipt and one record per session. Medulla verifies every record against its committed `RunTrace` and writes index nodes linked to the trace. An identical batch is idempotent across restart.

## Refusal codes

`SESSION_MANIFEST_UNSUPPORTED`, `SESSION_CONTENT_CHANGED`, `SESSION_TRACE_UNKNOWN` and `SESSION_CURSOR_STALE` on the search side. A cross-tenant reference reads as absent.

## Authoritative types

`HarnessSessionRecord` and `IndexHarnessSessionBatch` in `crates/medulla-types/src/harness_session.rs`.

## Test vectors

The round-trip vectors of R009.101.1 and the served tests of R009.101.1 and R009.101.2.
