# Seam contract: approved source query

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-FEDERATED-QUERY-R085
Consumer: scion-sdk SCION-SOURCE-INGEST-R004.7

## Provider

Repository: medulla. Rows: `MEDULLA-FEDERATED-QUERY-R085` and its children `MEDULLA-FEDERATED-QUERY-R085.1` to `MEDULLA-FEDERATED-QUERY-R085.8`. Medulla owns read-only query meaning and authorization. It issues an immutable `ApprovedSourceQuery`.

## Consumer

Repository: scion-sdk. Rows `SCION-SOURCE-INGEST-R004.7` to `SCION-SOURCE-INGEST-R004.12`. The SDK client executes the approved query through its registered adapter. It opens a read-only transaction (`SCION-SOURCE-INGEST-R004.8`). Medulla cannot enforce that inside the driver.

## Wire shape

| Method | Request | Answer |
|---|---|---|
| `ApproveSourceQuery` | `SourceQueryRequest`: typed parameters, dialect, source, schema and snapshot identities, authority, expiry, `SourceQueryLimits` | `ApprovedSourceQuery` |
| `VerifySourceQueryApproval` | `SourceQueryApprovalCheck` | `SourceQueryApprovalValidity` |

## Refusal codes

- `SOURCE_QUERY_APPROVAL_KIND`: the approval kind does not match the request.
- `unsupported_dialect`: the dialect has no approved renderer.
- A changed binding, authority, source or revision refuses. A revoked, expired or consumed approval refuses.

## Authoritative types

- `SourceQueryRequest`, `ApprovedSourceQuery`, `SourceQueryApprovalCheck`, `SourceQueryApprovalValidity` and `SourceQueryLimits`: `crates/medulla-result-types/src/source_query.rs` in medulla.

## Test vectors

- `crates/medulla-result-types/src/source_query.rs::tests::approval_records_round_trip` in medulla.
- `tests/source_query_approval_served.rs::approval_to_verification_journey` in medulla.
