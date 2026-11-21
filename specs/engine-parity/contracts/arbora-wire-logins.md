# Seam: outside SQL client identities

Provider: medulla MEDULLA-ENGINE-PARITY-R007.4
Consumer: arbora ARBORA-OPS-R858.1

## Wire shape

Arbora calls the existing served method `Identity` with the ops `grant_wire_login` (`IdentityOp::WireLogin(WireLoginOp::Grant)`) and `revoke_wire_login` (`WireLoginOp::Revoke`), both needing the exact scope `identity:admin` from a direct actor; the record's tenant is the verified actor's tenant. A grant carries `login`, `kind` (`Password`, `Certificate` or `Token`), the verifier and the engine principal it maps to, and returns `WireLoginGranted { login, kind, principal, tenant, created_ms }`. A revoke names the login and returns `WireLoginRevoked { login, revoked_ms }`. A grant identical to an existing record and a revoke of a revoked login return the stored record unchanged. Medulla never reads a principal from the request body and never stores a secret: the verifier is a SCRAM salted credential, a certificate digest or a token digest, and a malformed verifier refuses `IDENTITY_INVALID`.

## Refusal codes

`WIRE_LOGIN_EXISTS` when a grant names an existing login with a different principal, kind or verifier, `WIRE_LOGIN_UNKNOWN` when a revoke names no login, and `INVALID_LOGIN` (SQLSTATE `28000`) at connect time for an unknown, revoked or mismatched login.

## Authoritative types

`WireLoginRecord` and `WireLoginKind` in `crates/medulla-identity-types/src/identity/wire_login.rs`; the operation is `IdentityOp::WireLogin` in `crates/medulla-identity-types/src/identity/ops.rs`.

## Test vectors

`tests/identity/test_wire_login_served.py` holds the served vectors that both sides test against.
