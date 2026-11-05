# Seam: connector conformance evidence

Provider: medulla MEDULLA-TYPED-PACKS-R807
Consumer: arbora ARBORA-OPS-R067.10

## Wire shape

Arbora calls the served `ConnectorPack` read `ConformanceEvidence` with a tenant and a connector. Medulla returns the current immutable `ConformanceEvidence` record (`pack_digest`, `tool_fingerprints`, `issued_at`, `expires_at`, `revoked_at`) or refuses. The write path is the connector onboarding import (`MEDULLA-TYPED-PACKS-R103`); arbora never writes the record.

## Refusal codes

`CONFORMANCE_EVIDENCE_ABSENT`, for a missing, expired, revoked or other-tenant record.

## Authoritative types

`ConformanceEvidence` in `crates/medulla-catalog-types/src/connector_pack/conformance_evidence.rs` (new) and the `ConnectorPackOp::ConformanceEvidence` variant in `crates/medulla-catalog-types/src/connector_pack/ops.rs`.

## Test vectors

`tests/packs/test_conformance_evidence_served.py::test_evidence_written_read_revoked_and_tenant_scoped` is the served vector.
