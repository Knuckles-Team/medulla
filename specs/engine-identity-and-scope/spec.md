# MEDULLA-IDENTITY — Engine identity, mode and scope authority

**Owner:** Medulla (native Rust) · Cross-repo IDs: Arbora (`ARBORA-IDENTITY-R014`, `ARBORA-IDENTITY-R019.3`, `ARBORA-IDENTITY-R026.2`, `ARBORA-IDENTITY-R029.6`, `ARBORA-IDENTITY-R651`, `ARBORA-IDENTITY-R652`).

## Outcome and boundary

An operator can configure several trusted identity issuers, modify engine roles, and transition identity modes without losing administrative access or allowing an unverified caller to widen authority. Clients can discover the exact engine scope registry from the public contract. A contributor can implement and test every requirement from this directory and a fresh public checkout.

## User Scenarios & Testing

### User Story 1 - Trust several issuers and keep role changes atomic (Priority: P1)

The user is an operator. The outcome is that several identity issuers are trusted with per-issuer kind limits and that every role change is audited with the write, so that no unverified caller gains authority and no change is lost or unrecorded.

**Acceptance scenarios**

1. **Given** an ordered trust list of two issuers, **When** a token from an unknown, ambiguous or disallowed-kind issuer arrives, **Then** verification fails before method dispatch.
2. **Given** a role change whose persistence fails, **When** the transaction aborts, **Then** the role set and its audit entry roll back together.

Independent test: `MEDULLA-IDENTITY-R001` and `MEDULLA-IDENTITY-R002`.

### User Story 2 - Transition the identity mode without losing an admin (Priority: P1)

The user is an operator. The outcome is a compare-and-set mode transition that keeps accounts, links, grants and data, invalidates old sessions and refuses when no admin would survive.

**Acceptance scenarios**

1. **Given** a `local` store with an admin and a configured provider, **When** the transition to `external` commits, **Then** the epoch advances once and sessions of the older epoch are refused.
2. **Given** a stale epoch or no surviving admin, **When** the transition runs, **Then** it refuses with a typed code and the mode and epoch are unchanged.

Independent test: `MEDULLA-IDENTITY-R003.8`.

### User Story 3 - Run in an isolated anonymous store (Priority: P2)

The user is a local developer. The outcome is an explicit anonymous mode over loopback or a private socket that fixes one principal and tenant and refuses everything else.

**Acceptance scenarios**

1. **Given** `--auth-mode none` on a fresh store, **When** a client creates, writes, reads and restarts, **Then** the fixed anonymous authority applies throughout.
2. **Given** a remote listener or an authenticated store, **When** the engine starts in this mode, **Then** it refuses to start.

Independent test: `MEDULLA-IDENTITY-R003.1` and `MEDULLA-IDENTITY-R003.3`.

### User Story 4 - Classify scopes and bound a run's scope by policy (Priority: P1)

The user is a policy owner. The outcome is one generated scope registry and a stored policy node that no model-written plan can widen.

**Acceptance scenarios**

1. **Given** the capability declarations, **When** the registry is generated, **Then** every scope has one class and the digest matches the server.
2. **Given** a plan whose tool set exceeds the stored bound, **When** `AgentAssemble` runs, **Then** it refuses with `SCOPE_EXCEEDS_POLICY` and writes one audit entry.

Independent test: `MEDULLA-IDENTITY-R004` and `MEDULLA-IDENTITY-R008.6`.

### User Story 5 - Read only the knowledge units a principal may see (Priority: P1)

The user is a principal reading stored chats, code and memories. The outcome is that a read returns only the current version of units the principal owns or that are shared into its scope, with tenant chains, audited reads and typed metadata.

**Acceptance scenarios**

1. **Given** a private unit and an org-shared unit, **When** a principal of the same tenant reads, **Then** only the org-shared current version returns and a principal of another tenant sees neither.
2. **Given** a point read of a hidden or absent unit, **When** it runs, **Then** both return the same `UNIT_OUT_OF_SCOPE` code.

Independent test: `MEDULLA-IDENTITY-R023.6` and `MEDULLA-IDENTITY-R610.6`.

### User Story 6 - Share, own and claim through served operations (Priority: P2)

The user is an owner or a tenant admin. The outcome is that a share grant or revoke, and an inferred ownership plan with exact rollback, persist across restart.

**Acceptance scenarios**

1. **Given** a node shared to `org` and then revoked, **When** the engine restarts, **Then** a second principal does not see the node.
2. **Given** an applied ownership plan, **When** it is rolled back, **Then** the prior role set returns exactly.

Independent test: `MEDULLA-IDENTITY-R023.13.6` and `MEDULLA-IDENTITY-R700.6`.

### User Story 7 - Resolve, inspect and renew sessions and approval keys (Priority: P2)

The user is the serving gateway. The outcome is durable session records, a token-free liveness check, a compare-and-set renewal and registered approval keys.

**Acceptance scenarios**

1. **Given** a live session, **When** liveness is asked by opaque reference, **Then** the reply is exactly one of `live`, `revoked`, `expired` or `not_session_bound` and no token travels.
2. **Given** a stale authority revision, **When** renewal runs, **Then** it refuses with `PreconditionFailed` and mutates nothing.

Independent test: `MEDULLA-IDENTITY-R709` and `MEDULLA-IDENTITY-R710`.

### Edge Cases

- An ambiguous key or issuer match fails verification before method dispatch; an expired overlap key fails after refresh.
- A transition whose source or target is `none` refuses; conversion into or out of `none` is an offline operator command.
- A principal with no policy node gets the empty bound, so every tool is out of bound.
- A hidden and an absent unit return identical codes and messages.
- A full read-audit queue refuses the read with `AUDIT_UNAVAILABLE`.
- A plan that removes the last admin or widens beyond the tenant is refused and leaves durable state unchanged.

### Assumptions

- Questions and tests use locally signed JWTs, local JWKS fixtures, disposable redb files and a test gateway attestation signer; no live identity provider enters the pull-request gate.
- Arbora owns public admission and mode selection; Medulla owns durable state, verification and the engine scope registry.
- The replicated tenant provisioning ordering gap named in [test-spec.md](test-spec.md) is inherited and is not claimed as fixed here.

## Requirements

- **FR-001**: The primary verifier trusts an ordered multi-issuer list with per-issuer kind limits. Rows: `MEDULLA-IDENTITY-R001`.
- **FR-002**: Identity and RBAC writes recompute roles and audit atomically. Rows: `MEDULLA-IDENTITY-R002`.
- **FR-003**: The identity mode is a durable singleton with a compare-and-set transition. Rows: `MEDULLA-IDENTITY-R003.4`, `MEDULLA-IDENTITY-R003.5`, `MEDULLA-IDENTITY-R003.6`, `MEDULLA-IDENTITY-R003.7`, `MEDULLA-IDENTITY-R003.8`.
- **FR-004**: An explicit isolated anonymous store is available over loopback or a private socket. Rows: `MEDULLA-IDENTITY-R003.1`, `MEDULLA-IDENTITY-R003.2`, `MEDULLA-IDENTITY-R003.3`.
- **FR-005**: One generated scope registry classifies every capability scope. Rows: `MEDULLA-IDENTITY-R004`, `MEDULLA-IDENTITY-R007`.
- **FR-006**: Semantic workers use one identity and tenant roles read the commons graph. Rows: `MEDULLA-IDENTITY-R005`, `MEDULLA-IDENTITY-R006`.
- **FR-007**: A run's scope comes from a durable policy node. Rows: `MEDULLA-IDENTITY-R008.1`, `MEDULLA-IDENTITY-R008.2`, `MEDULLA-IDENTITY-R008.3`, `MEDULLA-IDENTITY-R008.4`, `MEDULLA-IDENTITY-R008.5`, `MEDULLA-IDENTITY-R008.6`.
- **FR-008**: Knowledge segments are principal-scoped with one sharing enum. Rows: `MEDULLA-IDENTITY-R023.1`, `MEDULLA-IDENTITY-R023.2`, `MEDULLA-IDENTITY-R023.3`, `MEDULLA-IDENTITY-R023.4`, `MEDULLA-IDENTITY-R023.5`, `MEDULLA-IDENTITY-R023.6`.
- **FR-009**: Typed principal metadata is admitted and persisted as versioned graph nodes. Rows: `MEDULLA-IDENTITY-R023.7`, `MEDULLA-IDENTITY-R023.8`, `MEDULLA-IDENTITY-R023.9`, `MEDULLA-IDENTITY-R023.10`, `MEDULLA-IDENTITY-R023.11`, `MEDULLA-IDENTITY-R023.12`.
- **FR-010**: Share grant and revoke persist across restart. Rows: `MEDULLA-IDENTITY-R023.13.1`, `MEDULLA-IDENTITY-R023.13.2`, `MEDULLA-IDENTITY-R023.13.3`, `MEDULLA-IDENTITY-R023.13.4`, `MEDULLA-IDENTITY-R023.13.5`, `MEDULLA-IDENTITY-R023.13.6`.
- **FR-011**: Tenant chains, secured reads and ownership claims are scoped and audited. Rows: `MEDULLA-IDENTITY-R610.1`, `MEDULLA-IDENTITY-R610.2`, `MEDULLA-IDENTITY-R610.3`, `MEDULLA-IDENTITY-R610.4`, `MEDULLA-IDENTITY-R610.5`, `MEDULLA-IDENTITY-R610.6`, `MEDULLA-IDENTITY-R706`.
- **FR-012**: Graph ownership is inferred and applied as reversible grants; delegation, policy bundles, resource access and client sessions are served. Rows: `MEDULLA-IDENTITY-R700.1`, `MEDULLA-IDENTITY-R700.2`, `MEDULLA-IDENTITY-R700.3`, `MEDULLA-IDENTITY-R700.4`, `MEDULLA-IDENTITY-R700.5`, `MEDULLA-IDENTITY-R700.6`, `MEDULLA-IDENTITY-R701`, `MEDULLA-IDENTITY-R702`, `MEDULLA-IDENTITY-R703`, `MEDULLA-IDENTITY-R704`, `MEDULLA-IDENTITY-R705`.
- **FR-013**: Identity domain operations, durable session records, liveness, renewal, approval keys, credential observation and reference inspection are served. Rows: `MEDULLA-IDENTITY-R707`, `MEDULLA-IDENTITY-R708`, `MEDULLA-IDENTITY-R709`, `MEDULLA-IDENTITY-R710`, `MEDULLA-IDENTITY-R711`, `MEDULLA-IDENTITY-R712`, `MEDULLA-IDENTITY-R713`.
- **FR-014**: Each lifecycle named in these rows is an explicit state machine with one transition function, one stored state field, a typed refusal of illegal transitions and an ontology description (organization constitution Article XXXI). Rows: `MEDULLA-IDENTITY-R720`, `MEDULLA-IDENTITY-R720.1`, `MEDULLA-IDENTITY-R720.2`, `MEDULLA-IDENTITY-R721`, `MEDULLA-IDENTITY-R721.1`, `MEDULLA-IDENTITY-R721.2`, `MEDULLA-IDENTITY-R722`, `MEDULLA-IDENTITY-R722.1`, `MEDULLA-IDENTITY-R722.2`.

## Success Criteria

- **SC-001**: An unknown, ambiguous or disallowed-kind issuer fails verification before method dispatch in 100 percent of cases (`MEDULLA-IDENTITY-R001`).
- **SC-002**: A mode transition with a stale epoch or no surviving admin leaves the mode and epoch unchanged in 100 percent of cases (`MEDULLA-IDENTITY-R003.8`).
- **SC-003**: A plan beyond the stored bound is refused with an audit entry (`MEDULLA-IDENTITY-R008.5`).
- **SC-004**: A shared-then-revoked node is invisible to a second principal after restart (`MEDULLA-IDENTITY-R023.13.6`).
- **SC-005**: No session or approval-key operation returns or logs a raw token (`MEDULLA-IDENTITY-R709`, `MEDULLA-IDENTITY-R711`).

## Typed principal metadata

Principal preferences, client descriptions and interaction networks use typed graph records under R023.
Canonical identity records still own authentication and grants. Metadata references those records without replacing their authority.
Untyped writes must refuse before mutation. Legacy file imports require explicit mapping, provenance and the same graph admission.

## Public interfaces and ownership

Medulla owns token verification, durable engine RBAC/mode state, capability scope declarations, method-level authorization and generated scope catalog. A serving gateway supplies a **signed, nonce-bound attestation** of loopback binding and fresh MFA when a mode transition requires facts Medulla cannot observe directly. Medulla verifies the attestation signature, audience, expiry, tenant, principal, nonce and one-time use before the transition; caller-supplied booleans are never trusted. The gateway may narrow access but cannot replace Medulla's durable role or scope authority. Client applications use the generated registry; they cannot manually extend the engine registry.

The `none → local` transition requires a credentialed human admin; `none → external` requires an enabled IdP and linked human admin; `local → external` requires an enabled admin mapping; `external → local` requires local credentials for every retained admin. Any `→ none` transition requires an explicitly acknowledged operation, `identity:admin`, fresh MFA attestation and loopback binding attestations for both engine and gateway. A mode change cannot delete accounts, policy or audit history.

## Design and quality rules

Reuse `crates/medulla-server-api/src/oidc.rs` and `crates/medulla-server-auth/src/auth.rs` for JWT binding, `crates/medulla-isolation/src/isolation/identity_admin.rs` and `crates/medulla-server-router/src/dispatch/router/identity_access.rs` for RBAC mutations, `crates/medulla-isolation/src/rbac_persist.rs` for durable control state, `crates/medulla-kernel/src/audit.rs` for the existing hash-chain algorithm, and `crates/medulla-capabilities/src/domains/security.rs` plus `gen_contract` for scope generation. Extend these authorities; do not add a second identity database or independent scope allowlist.

Tests cover both accepted and denied paths and exact restart behavior. Run CCCC, KISS, jscpd and Dupehound on changed code, with no new clone authority. Baseline PR tests use locally signed JWTs, local JWKS fixtures, disposable redb files and a test gateway attestation signer. No live IdP, private deployment or manually provisioned service is required. A public deployment smoke remains separate release evidence.

## Traceability

See [test-spec.md](test-spec.md). The design is in [plan.md](plan.md); entities are in [data-model.md](data-model.md); the shortest proof is in [quickstart.md](quickstart.md).
