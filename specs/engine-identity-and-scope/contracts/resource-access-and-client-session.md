# Seam: resource access decision and client session vocabulary

Provider: medulla MEDULLA-IDENTITY-R704
Consumer: arbora ARBORA-IDENTITY-R019.3

## Wire shape

The Arbora `fleet.call` gate calls `Method::CheckResourceAccess { agent_id, resource_ref, protocol, operation }` and receives `ResourceAccessDecision`; the MCP session open (`ARBORA-IDENTITY-R026.2`) writes a `ClientSession` node whose principal must equal the verified caller (`MEDULLA-IDENTITY-R705`). The decision is read-only and defaults to deny.

## Refusal codes

`RESOURCE_GRANT_MISSING`, `RESOURCE_UNKNOWN`, `GRANT_EXPIRED`, `TENANT_MISMATCH` and `CLIENT_SESSION_PRINCIPAL_MISMATCH`.

## Authoritative types

`ResourceAccessDecision` in `crates/medulla-types/src/acl.rs` (new); the `ClientSession` label in the identity pack ontology (new).

## Test vectors

`crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/resource_access.rs` and `client_session.rs` (planned).
