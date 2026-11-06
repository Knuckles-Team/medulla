# MEDULLA-IDENTITY — Data model

This spec adds identity, scope and metadata types to the engine. Types marked new do not exist yet; the owning row names their exact file.

## Entities

### Issuer trust entry

- Fields: `issuer`, `jwks_source`, `audience`, `allowed_kind`, `overlap_until` (`MEDULLA-IDENTITY-R001`, `crates/medulla-server-api/src/oidc.rs`).
- Rule: an ordered list; an ambiguous or disallowed-kind match fails before dispatch.

### Identity mode record

- Fields: the existing `AuthMode { None, Local, External }`, `IdentityConfig { mode, epoch, issuer_kid_current, local_fallback }` and `ConfigOp::Transition { request: ModeTransition { expected_epoch, to, issuer_kid, local_fallback } }` (`MEDULLA-IDENTITY-R003.4`, `crates/medulla-identity-types/src/identity/store/modes.rs`).
- Rule: a transition commits by compare-and-set on the epoch (`EpochConflict` on a mismatch); a transition to or from `None` refuses `MODE_TRANSITION_FORBIDDEN`; every session and envelope carries `mode_epoch`.

### Scope bound and run-scope policy node

- Fields: `ScopeBound { policy_iri, allowed_tools, allowed_graphs }` (exists, `crates/medulla-isolation/src/isolation/scope_bound.rs`); graph node class `:RunScopePolicy` with `allowed_tools` and `allowed_graphs` (`MEDULLA-IDENTITY-R008.4`, ontology `core-foundation-v1.ttl`).
- Rule: `policy_iri` is never empty; a missing node is the empty bound.

### Knowledge unit and sharing

- Fields: `KnowledgeUnit { id, owner, version, supersedes, access_labels, share_scope }`, `ShareScope { private, org, commons }`, `UnitPrincipal` (exist, `crates/medulla-identity-types/src/identity/segment.rs`); `RbacAdminOp::ShareGrant { node_id, scope, grantees }` and `ShareRevoke { node_id }` (`MEDULLA-IDENTITY-R023.13.1`, `crates/medulla-types/src/acl.rs`, new variants).
- Rule: `private` and `commons` take no grantees; `grantees` holds at most 64 ids; node keys are `_owner_id`, `_shared_scope`, `_grants`, `_commons_copy`.

### Typed principal metadata

- Fields: `TypedMetadata` of `Preference`, `ClientMetadata` and `InteractionNetwork`, `PrincipalRef` (`MEDULLA-IDENTITY-R023.7`, `crates/medulla-identity-types/src/identity/metadata.rs`, new).
- Rule: every type has `deny_unknown_fields` and `schema_version`; field bounds are stated in the row.

### Tenancy

- Fields: `TenantParent`, `AccessibleGraphs`, `Marking`, `ReadAudit`, `OwnershipClaim`, `ClaimPreview`, `TenantPredicate` (exist, `crates/medulla-identity-types/src/tenancy.rs`).
- Rule: identifiers are validated at construction; an unscopable query is refused.

### Ownership plan

- Fields: `GraphOwnershipSignals`, `OwnershipDisposition`, `GrantPlanEntry`, `RollbackEntry`, `plan_digest` (`MEDULLA-IDENTITY-R700.1`, `crates/medulla-identity-types/src/identity/ownership.rs`, new).
- Rule: at most 256 writers and 64 roles per entry; rollback restores the exact prior role set.

### Delegation, policy bundle, resource access and client session

- Fields: `DelegationRecord` (`MEDULLA-IDENTITY-R701`), `PolicyRuleBundle { schema_version, bundle_id, scope, rules }` (`MEDULLA-IDENTITY-R702`), `ResourceAccessDecision` (`MEDULLA-IDENTITY-R704`), the `ClientSession` label (`MEDULLA-IDENTITY-R705`).
- Rule: a bundle binds one scope at most; a decision defaults to deny.

### Session records and approval keys

- Fields: `SessionRecordProjection`, `SessionOp::Liveness`, `InspectReference`, `RenewReference`, `RegisterApprovalKey` (`MEDULLA-IDENTITY-R708` to `MEDULLA-IDENTITY-R713`).
- Rule: no operation takes, returns or logs a raw token; renewal requires the exact global revision.

## Term decisions

A term is reused from an open vocabulary when an equivalent exists; otherwise it is minted in the owning core module with a `skos:exactMatch` or `skos:closeMatch` mapping, and every new class extends a core class.

- `ClientSession` (`MEDULLA-IDENTITY-R705`): minted in the identity pack, extends the engine core session class, `skos:closeMatch prov:Activity`. Its properties map as `principal` to `prov:wasAssociatedWith` (`skos:closeMatch`), `started_at_ms` to `prov:startedAtTime` and `ended_at_ms` to `prov:endedAtTime` (`skos:closeMatch`; the values are epoch milliseconds, not `xsd:dateTime`).
- `RunScopePolicy` (`MEDULLA-IDENTITY-R008.4`): minted in `core-foundation-v1.ttl`, extends the core policy class, `skos:closeMatch odrl:Policy`. `allowed_tools` and `allowed_graphs` have no open-vocabulary equivalent.
- `identity_metadata` module (`MEDULLA-IDENTITY-R023.8`): minted with `Preference`, `ClientMetadata` and `InteractionNetwork`, each extending a core class; `InteractionNetwork` carries `skos:closeMatch foaf:knows` on its relation edge. `Preference` and `ClientMetadata` have no open-vocabulary equivalent.

## State machine types

- Rows `MEDULLA-IDENTITY-R720`, `MEDULLA-IDENTITY-R721`, `MEDULLA-IDENTITY-R722`: each machine type stores one state field and exposes one `apply` (or `step`) function that returns `TransitionRefused` for an illegal transition.
- Deleted fields: the dual-truth flags and literals named in the rows (for example `terminal`, `tombstone`, `reclaimed`, `cancel_requested`, `active`, `revoked`, `destination_installed`).

### State machine fields

- `GovernedChangeMachine`. States: `Proposed`, `Approved`, `Consumed`, `Revoked`, `Expired`. Persisted state: `status` (enum `GovernedStatus`, `crates/medulla-mutation-types/src/governed_change.rs:117`). `effective_status(now)` derives time expiry for reads.
- `UserStatusMachine`. States: `Active`, `Disabled`, `PendingReset`, `PendingVerification`, `Deprovisioned`. Persisted state: `status` (enum `UserStatus`, `crates/medulla-identity-types/src/identity/model.rs:22`). `active` and `revoked` are derived from it.
- `RbacElevationMachine`. States: `Requested`, `Active`, `Revoked`, `Expired`. Persisted state: `status` (enum `ElevationStatus`, `crates/medulla-identity-types/src/rbac_elevation/ledger.rs:31`). `is_live` is derived.
