# Seam: served share grant and revoke

Provider: medulla MEDULLA-IDENTITY-R023.13
Consumer: arbora ARBORA-IDENTITY-R651

## Wire shape

Arbora calls `Method::RbacAdmin` with `{"ShareGrant":{"node_id","scope","commons","grantees"}}` or `{"ShareRevoke":{"node_id"}}` (`RbacAdminOp::ShareGrant(ShareGrant)` / `RbacAdminOp::ShareRevoke(ShareRevoke)`); `grantees` is at most 64 principal id strings and must be empty for `private` and when `commons` is true (`commons` defaults to false and requires scope `tenant`). Medulla commits through the graph gateway, appends a reservation and an outcome `OP_AUDIT` event to the graph's audit chain (R023.13.5) and replies with the acknowledgement text `share_granted` or `share_revoked`; the scope values are `private` and `tenant` (`placement` is refused); `commons` promotes a copy to the `__commons__` graph.

## Refusal codes

`ACCESS_DENIED` for a caller without owner or `kg:admin` authority, `AUTH_TENANT_MISMATCH` for a node in another tenant, `INVALID_ARGUMENT` for an absent node or an invalid scope; a node the caller cannot see answers with the exact text of an absent node (`INVALID_ARGUMENT: node '<id>' does not exist`); a refusal changes no state.

## Authoritative types

`RbacAdminOp`, `ShareGrant` and `ShareRevoke` in `crates/medulla-identity-core-types/src/acl.rs`; `ShareScope` in `crates/medulla-identity-core-types/src/identity/segment.rs`.

## Test vectors

`tests/identity/test_share_restart_served.py` (planned) holds the served vectors; the Rust round-trip vector is `crates/medulla-identity-core-types/src/acl.rs::tests::share_ops_round_trip` (planned).
