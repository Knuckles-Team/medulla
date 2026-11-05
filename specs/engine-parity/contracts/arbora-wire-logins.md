# Seam: outside SQL client identities

Provider: medulla MEDULLA-ENGINE-PARITY-R007.4
Consumer: arbora ARBORA-OPS-R858.1

## Wire shape

Arbora calls the served methods `WireLoginGrant` and `WireLoginRevoke` with the verified principal and tenant. A grant carries `login`, `kind` (`Password`, `Certificate` or `Token`), the verifier and the engine principal it maps to. A revoke names the login. Medulla never reads a principal from the request body and never stores a secret: the verifier is a SCRAM stored key, a certificate-key digest or a token digest.

## Refusal codes

`WIRE_LOGIN_EXISTS` when a grant names an existing login with a different principal, `WIRE_LOGIN_UNKNOWN` when a revoke names no login, and `INVALID_LOGIN` (SQLSTATE `28000`) at connect time for an unknown, revoked or mismatched login.

## Authoritative types

`WireLoginRecord` and `WireLoginKind` in `crates/medulla-identity-types/src/identity/wire_login.rs`; the operation is `IdentityOp::WireLogin` in `crates/medulla-identity-types/src/identity/ops.rs`.

## Test vectors

`tests/identity/test_wire_login_served.py` holds the served vectors that both sides test against.
