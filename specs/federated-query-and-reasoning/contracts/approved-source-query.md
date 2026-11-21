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
| `ApproveSourceQuery` | `request: ApproveSourceQueryRequest { tenant, query: SourceQueryRequest }` (typed parameters, dialect, source, schema and snapshot identities, expiry, `SourceQueryLimits`) and `source_approval: ConnectorPackSourceApprovalResult`. `admission` is engine-stamped; a client value is overwritten. | `SourceQueryApprovalIssued { outcome, approved: Option<ApprovedSourceQuery>, changed_work_item_ids }` |
| `VerifySourceQueryApproval` | `request: SourceQueryApprovalCheck` and `source_approval: ConnectorPackSourceApprovalResult` | `SourceQueryApprovalValidity` |

The approval carries the exact SQL and parameters. The engine lease keeps only identity and digests. Verification must come from the same principal, agent and delegation chain that obtained the approval.

## Refusal codes

`SourceQueryError::code` gives `SOURCE_QUERY_` plus the upper-case kind. The reply text is the code, a colon and the `SourceQueryFailure`.

- `SOURCE_QUERY_UNSUPPORTED_DIALECT`, `SOURCE_QUERY_INVALID_QUERY`, `SOURCE_QUERY_LIMIT_EXCEEDED`: the request is refused before approval.
- `SOURCE_QUERY_UNAUTHORIZED`: the source is not approved and registered for the caller, a binding or digest changed, or the verifier is another identity.
- `SOURCE_QUERY_STALE_SOURCE`: the receipt or observed source, schema or snapshot identity differs from the approved one.
- `SOURCE_QUERY_APPROVAL_REVOKED`, `SOURCE_QUERY_APPROVAL_EXPIRED`: the lease is revoked or consumed, or expired.
- `SOURCE_QUERY_VERIFICATION_UNAVAILABLE`: the lease store or the Agent Library is unavailable.
- `SOURCE_QUERY_QUERY_FAILED`, `SOURCE_QUERY_CANCELLED`: SDK-side execution outcomes.
- `ACCESS_DENIED`: a generic `IssueControlLease` of `source.query.approval`, or a caller without read authority.

## Authoritative types

- `SourceQueryRequest`, `ApprovedSourceQuery`, `SourceQueryApprovalCheck`, `SourceQueryApprovalValidity` and `SourceQueryLimits`: `crates/medulla-result-types/src/source_query.rs` in medulla.
- `ApproveSourceQueryRequest`, `SourceQueryAdmission`, `SourceQueryApprovalGrant` and `SourceQueryApprovalIssued`: `crates/medulla-work-types/src/source_query_approval.rs` in medulla.

## Test vectors

- `crates/medulla-result-types/src/source_query.rs::tests::approval_records_round_trip` in medulla.
- `crates/medulla-h-admin/src/admin/connector_pack/tests/source_approval.rs::approval_to_verification_journey` in medulla.
