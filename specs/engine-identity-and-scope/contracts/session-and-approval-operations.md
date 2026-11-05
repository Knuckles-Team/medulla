# Seam: session reference, liveness, approval keys and credential observation

Provider: medulla MEDULLA-IDENTITY-R708
Consumer: arbora ARBORA-IDENTITY-R029.6

## Wire shape

Arbora's verifier calls `Method::Identity` with `SessionOp::Resolve` (optional `session_record`, `refresh: false` to observe), `SessionOp::Liveness`, `InspectReference`, `RenewReference` and the approval-key operations (`MEDULLA-IDENTITY-R709` to `MEDULLA-IDENTITY-R713`). Every reply names an opaque reference, never a raw token.

## Refusal codes

`SESSION_REFERENCE_UNKNOWN` for an unknown reference or one outside the deployment tenant, `PreconditionFailed` for a stale revision, scope change, expiry or revocation; a refusal mutates nothing.

## Authoritative types

`SessionRecordProjection` in `crates/medulla-identity-types/src/identity/projection.rs`; `SessionOp` in `crates/medulla-identity-types/src/identity/ops.rs`.

## Test vectors

`tests/identity/test_session_liveness_served.py`, `test_session_reference_served.py`, `test_session_record_served.py`, `test_credential_observe_served.py` and `test_approval_key_served.py` (all planned).
