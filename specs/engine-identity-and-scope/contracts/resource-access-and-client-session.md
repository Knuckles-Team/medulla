# Seam: resource access decision and client session vocabulary

Provider: medulla MEDULLA-IDENTITY-R704
Consumer: arbora ARBORA-IDENTITY-R019.3

## Wire shape

The Arbora `fleet.call` gate calls `Method::CheckResourceAccess { agent_id, resource_ref, protocol, operation }` (`protocol` is `mcp` or `a2a`) and receives `ResourceAccessDecision { allow, grant_id, scope_ceiling, expires_at_ms, revocation_revision, reason_code }`; grants are written by an administrator through `Method::RbacAdmin` with `RbacAdminOp::GrantResource` and `RevokeResource`; the MCP session open (`ARBORA-IDENTITY-R026.2`) writes a `ClientSession` node whose principal must equal the verified caller (`MEDULLA-IDENTITY-R705`). The decision is read-only and defaults to deny.

## Refusal codes

Decision reasons (in the reply, `allow: false`): `RESOURCE_UNKNOWN`, `RESOURCE_GRANT_MISSING`, `GRANT_EXPIRED`. A context for another tenant is refused at the door with `AUTH_TENANT_MISMATCH`. Client session: `CLIENT_SESSION_PRINCIPAL_MISMATCH`.

## Authoritative types

`ResourceAccessDecision`, `ResourceGrant` and `ResourceProtocol` in `crates/medulla-identity-types/src/acl.rs` (new); the `ClientSession` label in the identity pack ontology (new).

## Test vectors

`crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/resource_access.rs` and `client_session.rs` (planned).
