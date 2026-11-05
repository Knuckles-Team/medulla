# Seam: served catalog and schema operations

Provider: medulla MEDULLA-UNIFIED-DATA-PLANE-R765
Consumer: arbora ARBORA-OPS-R067

## Wire shape

Arbora sends every call with `{tenant_id, actor_id, source_id, trace_id, policy_context}`. Query calls add `{consistency, last_source_position, timeout}`. Medulla serves list, get, probe, share and unshare of the foreign source catalog, and `schema_context` answers (`MEDULLA-UNIFIED-DATA-PLANE-R029.2`). Arbora propagates the envelope unchanged and renders the typed route and freshness errors. A source ID refers to the Medulla registry; no caller supplies a DSN or a secret value. A position token is opaque to the consumer and scoped to one source and one subscription.

## Refusal codes

`DENIED`, `SOURCE_UNAVAILABLE`, `STALE_TIMEOUT`, `SCHEMA_DRIFT`, `CAPABILITY_UNSUPPORTED` and `OUTCOME_INDETERMINATE`. Arbora preserves the code and never turns a refusal into an empty result.

## Authoritative types

`ForeignSourceSpec` in `crates/medulla-plan`, the source record of `crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs` and `SchemaContextCatalog` in `crates/medulla-types/src/schema_context.rs`.

## Test vectors

The served tests named by the provider rows are the vectors: a wrong tenant, a stale copy, a missing source and a missing audit reservation each fail closed, and a valid schema answer names owner, joins, class and impact. The consumer proves its side against the same request and response types.
