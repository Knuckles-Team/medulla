# Seam: identity mode transition and credential operations

Provider: medulla MEDULLA-IDENTITY-R003
Consumer: arbora ARBORA-IDENTITY-R014

## Wire shape

Arbora serves the mode transition `ConfigOp::Transition { request: ModeTransition { expected_epoch, to, issuer_kid, local_fallback } }` and the credential operations through `Method::Identity { op }` (scope `identity:admin`; (`UserOp::Create`, `CredentialOp::ChangePassword`, `TokenOp::IssuePasswordReset`, `MEDULLA-IDENTITY-R707`). Medulla replies with the new epoch. The password is verified with argon2id at the request boundary and the store never sees plaintext.

## Refusal codes

Served: `IDENTITY_EPOCH_CONFLICT`, `IDENTITY_PRECONDITION_FAILED`, `IDENTITY_ILLEGAL_TRANSITION`, `MODE_TRANSITION_FORBIDDEN` (any edge to or from `none`) and `SESSION_EPOCH_STALE`. Offline conversion out of `none` (operator command, not wire): `IDENTITY_MODE_NOT_CONFIRMED` and `IDENTITY_MODE_NOT_ANONYMOUS`. Each refusal changes no state.

## Authoritative types

`IdentityOp` in `crates/medulla-identity-types/src/identity/ops.rs`; `AuthMode`, `IdentityConfig` and `ModeTransition` in `crates/medulla-identity-types/src/identity/config.rs`.

## Test vectors

`tests/identity/test_identity_mode_served.py` (planned) holds the served round trip that both sides test against.
