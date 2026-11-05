# Seam: source ingest of fetched pages and outside quality results

Provider: medulla MEDULLA-UNIFIED-DATA-PLANE-R651.7.3
Consumer: scion-sdk SCION-SOURCE-INGEST-R001

## Wire shape

A connector fetches one page or one outside quality result and submits it through the served `SourceIngest` method. A fetched page enters as one Document and is then enriched through the served `EnrichDocument` method (`MEDULLA-UNIFIED-DATA-PLANE-R651.7`). An outside quality result enters as a quality score that opens an incident when it fails (`MEDULLA-UNIFIED-DATA-PLANE-R762.8`). The envelope is the shared `{tenant_id, actor_id, source_id, trace_id, policy_context}`.

## Refusal codes

`DOCUMENT_NOT_FOUND` for a missing or invisible Document, `DENIED` for a caller outside the scope, and `SCHEMA_DRIFT` for a result that does not match the registered shape. A refusal stores nothing.

## Authoritative types

`EnrichDocument` and its receipt in `crates/medulla-source-types/src/source_ingestion/web_document.rs`.

## Test vectors

The served tests of `MEDULLA-UNIFIED-DATA-PLANE-R651.7.6` and `MEDULLA-UNIFIED-DATA-PLANE-R762.8` are the vectors. The consumer proves that its fetch result carries the same Document and result fields.
