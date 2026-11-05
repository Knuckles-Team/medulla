# Seam: served share grant and revoke

Provider: medulla MEDULLA-IDENTITY-R023.13
Consumer: arbora ARBORA-IDENTITY-R651

## Wire shape

Arbora calls `Method::RbacAdmin` with `RbacAdminOp::ShareGrant { node_id, scope, grantees }` or `ShareRevoke { node_id }`. Medulla commits through the mutation gateway, appends one audit event and replies with the committed revision; the scope values are `private`, `org` and `commons`.

## Refusal codes

`ACCESS_DENIED` for a caller without owner or `kg:admin` authority, `AUTH_TENANT_MISMATCH` for a node in another tenant, `INVALID_ARGUMENT` for an absent node or an invalid scope; a refusal changes no state.

## Authoritative types

`RbacAdminOp` in `crates/medulla-types/src/acl.rs`; `ShareScope` in `crates/medulla-identity-types/src/identity/segment.rs`.

## Test vectors

`tests/identity/test_share_restart_served.py` (planned) holds the served vectors; the Rust round-trip vector is `crates/medulla-types/src/acl.rs::tests::share_ops_round_trip` (planned).
