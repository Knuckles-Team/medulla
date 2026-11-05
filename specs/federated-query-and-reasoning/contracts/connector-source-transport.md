# Seam contract: connector source transport

Governing spec: [spec.md](../spec.md).

Provider: scion-sdk SCION-CONNECTOR-CONTROL-R069
Consumer: medulla MEDULLA-FEDERATED-QUERY-R073

## Provider

Repository: scion-sdk. Row `SCION-CONNECTOR-CONTROL-R069` supplies the transports and the contract terms of an operation-bound source: API, MCP, A2A and GraphQL.

## Consumer

Repository: medulla. Row `MEDULLA-FEDERATED-QUERY-R073` registers each source as a `ForeignSourceSpec` kind with declared capabilities. It reads a mapped entity through its discovered operation. Row `MEDULLA-FEDERATED-QUERY-R053.700` reads `Trino` and `Spark` sources in the same way.

## Wire shape

| Item | Shape |
|---|---|
| Source registration | `ForeignSourceSpec` with kind, declared capabilities and an operation per mapped entity |
| Key filter | pushed only when the source declares the capability; the exact local residual always stays |
| Submission | `SubmissionId` is the SHA-256 digest of preset id, input digests and parameters |

## Refusal codes

- `FOREIGN_BUDGET_EXCEEDED`: a per-query limit is spent.
- `OUTBOUND_TARGET_DENIED`: the target fails the shared outbound guard.
- `FOREIGN_SOURCE_UNGRANTED`: the caller has no grant for the source.
- `FENCE_FIELD_MISSING`, `SNAPSHOT_UNKNOWN`, `PRESET_UNAVAILABLE`: refusals of the `Trino` and `Spark` kinds.

## Authoritative types

- `ForeignSourceSpec`: `crates/medulla-types/src/wire` in medulla.
- `ForeignSourceRegistry`: `crates/medulla-federation/src/federation.rs` in medulla.

## Test vectors

- `tests/query/test_virtual_mapping_pushdown_served.py::test_pushed_and_unpushed_filters_equal_rows` in medulla.
- `tests/query/test_spark_trino_sources_served.py::test_resubmission_returns_same_submission` in medulla.
