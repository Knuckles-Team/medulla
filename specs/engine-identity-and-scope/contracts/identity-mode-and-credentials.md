# Seam: identity mode transition and credential operations

Provider: medulla MEDULLA-IDENTITY-R003
Consumer: arbora ARBORA-IDENTITY-R014

## Wire shape

Arbora serves `medulla.security.IdentityModeTransition { expected_epoch, to }` (scope `security:admin`) and the credential operations of `Method::Identity { op }` (`UserOp::Create`, `CredentialOp::ChangePassword`, `TokenOp::IssuePasswordReset`, `MEDULLA-IDENTITY-R707`). Medulla replies with the new epoch. The password is verified with argon2id at the request boundary and the store never sees plaintext.

## Refusal codes

`IDENTITY_MODE_STALE_EPOCH`, `IDENTITY_MODE_NO_SURVIVING_ADMIN`, `IDENTITY_MODE_NO_IDP`, `IDENTITY_MODE_OFFLINE_ONLY`, `IDENTITY_MODE_NOT_CONFIRMED` and `SESSION_EPOCH_STALE`; each refusal changes no state.

## Authoritative types

`IdentityOp` in `crates/medulla-identity-types/src/identity/ops.rs`; `IdentityModeRecord` in `crates/medulla-identity-types/src/identity/mode.rs` (new).

## Test vectors

`tests/identity/test_identity_mode_served.py` (planned) holds the served round trip that both sides test against.
