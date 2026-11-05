# MEDULLA-IDENTITY — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine verifies callers against an ordered multi-issuer trust list, mutates identity and RBAC state with the role recompute and the audit entry in one durable transaction, holds the identity mode as a compare-and-set singleton, classifies every capability scope in one generated registry, bounds a run's scope by a stored policy node, segments knowledge per principal with one sharing enum, and serves share, ownership, session-record and approval-key operations through the one identity method. This plan adds no second identity store, no second visibility store and no second policy path.

```text
request -> admission (crates/medulla-server-api/src/oidc.rs, crates/medulla-server-auth/src/auth.rs)
        -> Method::Identity / RbacAdmin   crates/medulla-server-router/src/dispatch/router/identity_access.rs
        -> IsolationLayer                 crates/medulla-isolation/src/isolation.rs
        -> durable policy store + audit   crates/medulla-kernel/src/audit.rs
```

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-isolation`, `crates/medulla-identity-types`, `crates/medulla-server-api`, `crates/medulla-server-auth`, `crates/medulla-server-router`, `crates/medulla-capabilities`, `crates/medulla-h-query`, `crates/medulla-h-decide`, `crates/medulla-h-admin`, `crates/medulla-kernel` and `crates/medulla-graph-core`. No new third-party dependency.
- **Storage**: The policy store (ControlRedb) holds the identity and RBAC state, the mode singleton and the audit chain; knowledge and metadata are graph nodes through the mutation gateway.
- **Testing**: Rust unit and served tests in the crates above, Python served tests under `tests/identity/`; locally signed JWTs, local JWKS fixtures and disposable redb files; no live identity provider or network in the pull-request gate.
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Handler, type and ontology additions to the existing engine crates.
- **Performance Goals**: Anonymous admission p99 at most 100 microseconds (`MEDULLA-IDENTITY-R003.2`); segment filter p95 below 2 ms at 10,000 units; scope injection p99 below 400 microseconds; share transition p99 below 10 ms; ownership plan p99 below 25 ms.
- **Constraints**: A refusal changes no state. Every authority comes from the verified request context, never from a request field. No version suffix in a name.
- **Scale/Scope**: 63 leaf rows over 7 user stories, 13 `FR-` labels and 5 `SC-` labels.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Identity, scope and metadata state live in the engine; Arbora composes the served operations.
- **R-II. Every wait has a deadline**: pass. The read-audit queue is bounded and a full queue refuses; no unbounded wait is added.
- **R-III. Every row sets budgets and names a benchmark**: pass. The hot-path rows name nightly bench groups with budgets.
- **R-IV. Crates are small, layered and cut over hard**: pass. State and transitions sit in `medulla-isolation` and `medulla-identity-types`; handlers sit in handler crates and never call a sibling handler.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries; Arbora owns admission.

## Project Structure

```text
crates/medulla-server-api/src/oidc.rs, anonymous.rs         existing: verifier and anonymous authority
crates/medulla-server-api/src/read_audit.rs                  new: read and claim audit sink
crates/medulla-server-auth/src/auth.rs                       existing: admission, mode epoch, issuer material
crates/medulla-isolation/src/isolation/                      existing: identity admin, scope_bound.rs; new: ownership.rs, share.rs, identity_store_admin/mode_authority.rs
crates/medulla-identity-types/src/identity/                  existing: ops, store, segment; new: mode.rs, ownership.rs, metadata.rs, delegation.rs
crates/medulla-identity-types/src/tenancy.rs                 existing: tenant tree, secured read, scope predicate
crates/medulla-h-query/src/query/segment_filter.rs           new: visible-unit filter and point-read check
crates/medulla-h-admin/src/admin/ownership.rs                new: ownership plan handler
crates/medulla-server-router/src/dispatch/router/identity_access.rs  existing: Identity, RbacAdmin and access arms
crates/medulla-capabilities/src/                             existing: scope registry and method descriptors
tests/identity/                                              new: served tests
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns that block planning; no research.md. The replicated tenant provisioning gap named in [test-spec.md](test-spec.md) is an inherited limit that this work does not claim to fix.

## Phase 1: Design

The rules below bind every row.

1. **One identity store, one visibility store.** `row_visibility` in `crates/medulla-graph-core/src/rls.rs` reads the committed node keys; no second store exists.
2. **Authority comes from the verified context.** Principal, tenant and owner are never read from a request field or from query text.
3. **Fail closed.** A missing policy node is the empty bound; an unscopable query is refused; a refusal changes no state.
4. **Audit with the write.** Role changes, mode transitions, share changes and ownership applies append an audit entry in the same commit.
5. **Planned means planned.** Types and functions marked `new` in a row do not exist yet; the row names their exact path.

### Existing system and reuse

The identity store, the isolation layer, the scope registry and the policy store exist; rows extend them. `ScopeBound`, `scope_within_bound`, `KnowledgeUnit`, `ShareScope`, `visible_units` and the tenancy model of `crates/medulla-identity-types/src/tenancy.rs` already exist and are verified by their own tests.

### Architecture

#### Existing seams

| Component | Reuse |
|---|---|
| `JwtValidator::from_env_primary`, `decode_with`, `validate_claims` in `crates/medulla-server-api/src/oidc.rs` | Evolve single primary issuer into an ordered validated issuer registry; retain network-free `from_parts` for tests. |
| `primary_oidc_validator`, `bind_verified_identity` in `crates/medulla-server-auth/src/auth.rs` | Keep envelope subject/tenant binding after cryptographic verification. |
| `identity_access.rs`, `IsolationLayer::try_register_agent` | Route typed identity mutations through one ControlRedb transaction. |
| `rbac_persist.rs` | Store complete role set, mode singleton and epoch under one owner; migration is versioned and restart-safe. |
| `crates/medulla-kernel/src/audit.rs` | Reuse chain hash/verification pattern. Add identity audit rows to the RBAC control transaction, because graph audit currently covers a different store. |
| `domains/security.rs`, `gen_contract` | Emit the public scope registry and digest from the same capability declarations checked at dispatch. |

#### Token processing order

Parse bounded JWT header and claims without trusting them. Resolve exact issuer to one configured entry. Resolve key ID from that issuer's JWKS; a cache miss may trigger a bounded refresh, then failure if still absent. Verify algorithm allowlist and signature, expiry, issuer and audience. Resolve principal kind and enforce issuer's allowed kinds. Bind token subject and tenant to the authenticated envelope and call context. Require token roles/scopes to be a subset of current durable grants. Only then dispatch a method. A key rotation overlap has explicit start/end; removed keys stop authorizing immediately after the configured refresh boundary.

#### Control transaction and audit

`IdentityAdmin` loads the current full role set and mode/epoch under a write transaction. It checks principal and scope, then computes a replacement set. It writes identity state and the audit record, then commits. No response succeeds until durable commit. Denial records are bounded and rate-controlled. Refusals contain no raw tokens, passwords, credential-bearing JWKS URLs or role-target secrets. The identity audit chain has sequence, previous hash, record hash and canonical payload digest. A verification oracle reads from genesis to head and reports missing, reordered or altered records. Recovery after crash reads the last committed role/mode/audit head; it never infers a successful transition from a partially written external session-revocation task.

#### Mode state machine

| From → to | Preconditions |
|---|---|
| `none → local` | Credentialed human admin is created/verified in the same activation flow. |
| `none → external` | Enabled IdP and linked human admin verified before mode commit. |
| `local → external` | At least one enabled IdP maps to a human admin; local fallback remains until post-commit validation. |
| `external → local` | Each retained admin has a verified local credential before external dependence is removed. |
| `local/external → none` | Exact acknowledgment, human `identity:admin`, fresh MFA attestation, engine and gateway loopback attestations, and a transition nonce bound to expected epoch. |

The transition response contains old/new mode, epoch and revocation receipt. Session revocation/issuer rotation are durable follow-up effects with retry state. Until completed, the old sessions are denied by epoch at Medulla. Concurrent transitions race on epoch. One commits, others return typed conflict. A failed prerequisite changes neither mode nor epoch.

#### Scope registry

Capability descriptors declare exact scope and class. The generator validates uniqueness and coverage, emits the registry with methods/errors and publishes its digest in the wheel. Server authorization resolves only exact declared scopes. Delegation cannot convert service-only into a human grant; approval cannot be inferred from wildcard or broad graph admin. Finance, fleet and other domain scopes enter through their owning capabilities, and stale undeclared names are rejected during generation. A consumer may generate a local allowlist from this catalog but cannot claim a broader class.

#### Foreign-read scope classification (R004)

QueryForeignColumns declares federation:read as an engine user scope.
Federation administration retains the federation:admin admin scope, and wildcard
scope classification is rejected. Registry membership grants no scope to a
principal. Foreign reads still require verified owner isolation and kg:read.

### Semantic owner registry identity

The R005 registry must key the canonical persistence root, tenant, and binding.
Distinct roots must never share a semantic owner; aliases of one root must share its handle.
The existing activation grant and verified-principal checks remain mandatory.
`semantic_owner_registry_is_scoped_to_persistence_root` must prove root isolation and canonical alias reuse.
Native facade acceptance remains pending.

### Policy-bound run scope (R008)

Flow: stored policy node → `scope_within_bound` → `ScopeExceedsPolicy` refusal at plan admission.
The attach point is the plan-admission path beside `policy_admin.rs`; the first child row locates it.
The change reuses the existing verified-principal binding and the audit append from R002.
Decision: the engine reads the bound from the stored node, not from the plan body. A rejected alternative is deriving scope from the plan, because a model-written plan could then widen its own bound.
Handoff: Arbora must check the same bound when work enters; Meristem only applies the engine's refusal.

### Principal-scoped knowledge segments (R023)

Data flow: content-read wire method -> the retrieval function -> `visible_units` (`crates/medulla-identity-types/src/identity/segment.rs`) -> permitted current units plus an audit entry.

Types: `KnowledgeUnit`, `share_scope`. Attach point: the retrieval function in the content-read path. Production caller: the content-read handler, created by the Wiring row. Failure behavior: out-of-scope reads return `OutOfScope` with the id only.

Reused: the visibility pattern at `crates/medulla-identity-types/src/rbac_elevation/ledger.rs:268` and the audit append from R002. New: version lineage and share scope on stored units.

Decision: access labels live in the unit itself, so every reader enforces them. Rejected alternative: a separate access table, because it can diverge from the unit it guards.

### Typed principal metadata within R023

#### Model and authority

Medulla owns typed preferences, client descriptions and interaction networks as graph records.
The canonical identity store remains the sole principal, credential and grant authority.
`crates/medulla-identity-types/src/identity/model.rs:41` defines UserRecord and its stable principal_id.
`crates/medulla-identity-types/src/identity.rs:1` documents that authority and its RBAC projection.
Metadata references exact canonical principal IDs. It never infers identity from an alias, hash or display name.
A client description references its controlling principal. It does not register a principal or confer a credential.
Interaction links describe observed relationships. They never create membership, sharing grants or trust.
R023 sharing still requires an explicit authorized scope. Private scope is the default.

Extend the proposed `crates/medulla-identity-types/src/identity/segment.rs` model from R023.1.
Its metadata payload is a closed union: Preference, ClientMetadata or InteractionNetwork.
Common fields are node ID, owner principal, tenant, graph incarnation, revision, schema digest and provenance.
Preference contains a registered preference key and a value matching that key's declared type.
Value types are boolean, signed integer, bounded text and a registered enum choice.
ClientMetadata contains an opaque client ID, canonical controlling principal reference and a bounded display label.
InteractionNetwork contains typed member references and directed links with a registered relation kind and observed timestamp.
Each member reference distinguishes a canonical principal from a client metadata node.
Refuse dangling, wrong-kind and foreign-scope references before storing any node or link.
Bounds are 128 UTF-8 bytes per key, 1024 per label and 4096 per text value.
One payload has at most 64 members, 256 links and 64 KiB of encoded content.
Use checked revision counters, existing bounded containers and Digest256. Reject unknown fields and variants.
No payload field carries passwords, tokens, credential hashes, role grants or arbitrary authorization claims.

#### Behavior and schema admission

Use an engine-managed graph schema source named identity_metadata through the existing R051 schema composition authority.
The source declares the three variants, reference kinds, required fields and bounds.
Declare a principal-metadata graph domain at graph provisioning; the declaration requires current administrative authority.
Every node and edge in that domain must satisfy the metadata contract, even without a caller-supplied type label.
Removing a label, using raw properties or changing the schema source cannot bypass admission.
Refuse source detachment or weakening while retained metadata depends on it.
Generic graphs retain their existing contracts; they cannot masquerade as the authoritative principal-metadata domain.
Resolve references and validate the complete candidate state before committing an atomic MutationBatch.
Updates require the expected revision and schema digest. Concurrent mismatches refuse without partial changes.
Current authorization and schema changes invalidate stale admission evidence before commit.
Metadata reads use the graph authority and R023 principal scope; metadata cannot widen either.

#### Existing code and production wiring

The reviewed source is c0b952776129fd509b9f87b411a298593c2591ec.
Existing code: `crates/medulla-server-auth/src/auth.rs:901` binds the verified caller; `crates/medulla-server-auth/src/access.rs:702` checks graph access.
`crates/medulla-h-graph/src/graph_ops/gateway.rs:72` routes structural graph mutations through the existing commit gateway.
`crates/medulla-h-graph/src/graph_ops/gateway_graph.rs:151` handles ApplyMutation application.
`crates/medulla-server-api/src/batch_compile/compile.rs:126` defines compile_methods_with_outbox for the canonical write set.
`crates/medulla-server-mutation/src/mutation_batch/commit/` owns durable commit and projection publication.
`crates/medulla-h-graph/src/graph_ops/nodes.rs:85` serves GetNodeProperties through the authorized read path.
`crates/medulla-server-api/src/schema_compose.rs` uses validate_and_compose for existing named schema sources.
`crates/medulla-h-sparql/src/rdf/validation.rs:21` calls medulla_shacl::validate through the served ShaclValidate boundary.
ShaclValidate is read-only inspection. It is not a mandatory metadata admission gate today.
Reachable today: no. The mandatory metadata domain, typed variants and common admission predicate remain proposed.

The new predicate must run before compilation and again against the admitted schema and authority at commit.
Structural writes enter through AddNode, AddEdge and ApplyMutation at the existing gateway.
Query and RDF writes use their existing gateway routes and must reach the same predicate.
Audit direct and opaque batch paths; a path lacking validated metadata evidence must refuse for this domain.
No alternate GraphCore mutation, direct store write or caller-selected schema may bypass that check.
Use existing generated method contracts. Register new fields and typed refusal codes before exposing them.
No new client metadata store, identity registry, schema composer or public write method is required.

#### Refusal, migration and evidence

Check caller authority before resolving protected principal references or exposing their existence.
Denied existing and absent references return the same redacted error shape and no stored metadata.
Validate type, bounds, schema digest, ownership, reference scope and expected revision before any durable mutation.
A disabled or revoked actor cannot regain access through previously admitted metadata or an import receipt.
Refusal leaves graph records, edges, revisions, import receipts and mutation outbox rows unchanged.
Existing security denial auditing remains separate and must contain no metadata payload or protected reference details.

Imports from legacy files require an explicit versioned mapping into these same variants and admission rules.
Record source kind, logical source reference, source Digest256, mapping version and verified importing actor.
Record the schema digest, previous revision and commit receipt in the same admitted transaction.
The caller supplies candidate source evidence; Medulla checks the bytes and derives the stored digest.
The same import identity and content replay once. Reusing it with changed content refuses.
Invalid legacy records remain unimported; an import cannot retain an untyped fallback in the authoritative graph.
A bounded import batch commits all validated records or none. Large imports use explicit independent batches.
Readback after restart uses Medulla records, even when the source file disappears or changes.
An exported file is a snapshot, never write authority. Reimport requires fresh admission and an expected revision.
Migration never changes canonical identity records, grants, credentials or principal IDs.

#### Acceptance and ownership

The six new children require the planned positive and negative cases in test-spec.md.
No helper-only test, schema declaration or generated client establishes served acceptance.
R051 and R098 remain schema prerequisites; their typed-packs files and crosswalk work are unchanged.
Arbora may compose these engine results. Meristem applies them; clients cannot become metadata or identity authorities.

#### Preserve the R008.2 construction invariant

Keep `ScopeBound.policy_iri` private and expose only a borrowed `policy_iri()` accessor.
All public construction must pass through `ScopeBound::new`.
Reject blank policy IRIs before construction; callers cannot replace or clear the stored IRI.
Compile-fail examples must reject external struct literals and direct field mutation.
Existing subset checks and allowed-tool/graph fields keep their behavior.

#### Arbora curated operation scopes (R004)

Arbora owns `decide:commit`, `policy:read`, `security:read`, `swarm:read`,
`telemetry:read` and `usage:read`. Register `decide:commit` as service-only;
register the five read scopes as user class, eligible for explicitly authorized
humans, services and API keys. Registration grants nothing automatically.
Arbora must still require the exact scope and bind reads to the verified caller's
tenant; decision commits remain service-only. This decision permits authorized
automated policy/security inspection without conferring administrative authority.
Regenerate both contract catalogs and their receipt using `gen_contract`.
Installed-wheel Arbora acceptance remains separate consumer evidence.

### Served share grant and revoke (R023.13)

Replaces: `agent_utilities/knowledge_graph/core/tenant_sharing.py` (`share_with_org`, `make_private`, `promote_to_commons`, `_set_scope`). The harness keeps no share logic after this row lands.

Meristem acceptance case (in the Meristem repository): `test_org_share_revocation_persists_across_a_real_restart`.

Call path: served `RbacAdmin` -> `crates/medulla-server-router/src/dispatch/router/identity_access.rs` (arm at line 305)` -> `share_transition` (`crates/medulla-isolation/src/isolation/share.rs`, new) -> mutation gateway commit -> `row_visibility` (`crates/medulla-graph-core/src/rls.rs:146`) on every read.

Crate: `medulla-isolation` for the transition, `medulla-types` for the wire operations.

Data structures: grantee sets are sorted vectors of interned `u32` principal ids, capped at 64. The node keeps `_owner_id`, `_shared_scope` and `_grants`. Memory per operation is O(g).

Concurrency: the commit takes the graph's shard writer. Readers never take a global lock. They read the off-lock snapshot.

Persistence: the redb store through the mutation gateway. A restart replays nothing extra, because the scope is a node property.

Tenant isolation: the transition refuses a node outside the caller's tenant before any read of its properties.

Budget: p50 under 2 ms and p99 under 10 ms per operation on a 1,000,000-node graph.

References
- No paper is cited.

### Anonymous store authority (R003.1)

MEDULLA-IDENTITY-R003.1 adds an explicit, isolated anonymous durable-store authority for Arbora's local no-auth admission. Arbora owns public admission and mode selection; Medulla owns the fixed anonymous provenance record, store separation, client/native protocol projection and existing ACL/mutation machinery. Default verified-envelope/OIDC behavior remains unchanged. This narrow opt-in supersedes earlier unconditional no-development-escape wording only for fresh anonymous-marked local stores. No deployment, credential or mode migration is authorized.

Unsigned replay metadata uses `anon1.` plus a strict JSON object containing only a 1..256-byte UTF-8 `idempotency_key` in the legacy envelope slot named `auth_token`. It is not a credential and carries no identity, MAC or signature. Only explicit anonymous mode accepts it. Durable request provenance uses authentication_method `none`; verified modes reject that provenance.

The native local boundary supports explicit `--tcp-only --tcp-addr 127.0.0.1:PORT` for hosts without Unix-socket support. It never silently retries a denied bind; omitted `--tcp-only` retains the existing UDS plus optional TCP behavior. Anonymous mode still rejects hostname/wildcard/nonloopback addresses.

Native scheduled consumers derive the same server-owned anonymous origin and fixed tenant before subscribing, claiming or projecting work. They never read authenticated signer policy in explicit anonymous mode. Served coverage in `tests/test_native_no_auth_served.py` imports a real connector pack and requires its committed head to become visible in a nonempty projected graph-schema source, in addition to checking background health. The optional AST repository enrichment branch has mode-aware source coverage but is not included in the scoped local runtime receipt.

R003.1 call path: `src/main.rs` CLI mode selection -> `crates/medulla-server/src/server_startup.rs::validate_auth_mode` and store-marker check under the existing persistence lock -> `ServerState.local_anonymous` -> `crates/medulla-server-api/src/anonymous.rs::request_authority` -> the existing dispatch, ACL and mutation gateways. `AuthorityOrigin::AnonymousLocal` is a server-owned discriminator, never wire data. Storage stays in the existing redb/transaction paths; only an isolated store marker is added. Per-request metadata is bounded to 2 KiB, with a 256-byte correlation key; no per-principal unbounded cache is added. Transient replay keys include the method-body digest, while durable replay/conflict semantics remain in the existing kernel. Startup sets the mode once before listener/background tasks; request admission reads it without a new global lock. R003.2 owns measurement rather than inferring a latency budget from the functional fixture.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, producers first: issuer trust and RBAC audit, then the mode singleton and its preconditions, the scope registry, the run-scope bound, the knowledge segments and typed metadata, the share operations, the ownership plan and the session and approval-key operations. Rows that edit `identity_access.rs` or `server_error_code.rs` run in sequence; rows of different crates run in parallel. Delivery slices:

1. Define a versioned issuer-entry schema and validation for duplicate issuer, invalid URL, key ambiguity and unsupported algorithm. Upgrade primary OIDC loading while preserving separate ancillary protocol validators. Add local signed-JWT fixtures for two issuers and key rotation.
2. Add typed full-replacement identity mutation in the ControlRedb write transaction. Couple the identity audit record and chain head to that transaction; add `AuditVerify` and bounded denial storage. Route existing `RegisterIdentity` and `RbacAdmin` through the same mutation path.
3. Add the durable mode singleton and epoch CAS. Implement the precondition table, verified gateway attestation and nonce, session-epoch invalidation, revocation/rotation receipt and restart recovery. Keep account/link/grant/data records intact.
4. Classify scopes in capability declarations, generate the registry and package digest, then update server grant evaluation to use the generated classification. Add consumer contract tests using only the public wheel catalog.
5. Run focused and full checks in [test-spec.md](test-spec.md). Review exact source and generated-artifact diffs. Record a merged-head SHA plus hosted results in [tasks.md](tasks.md). Keep spec acceptance pending until consumer and release proof exists.

The first implementation slice should land issuer verification with negative tests, then transactional RBAC audit, then mode transitions, then registry. Each slice has a compatibility and rollback receipt. Do not enable a partially generated scope table or silently reinterpret an existing role set.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every ontology module that a row adds or changes still loads through `GraphSchemaSources::default()` and passes its shapes check.
- A public item added by a row has a non-test caller under a handler or server crate (`MEDULLA-CONTRACT-R055`).
- Served tests use locally signed credentials and need no live identity provider or network.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- Request admission: `crates/medulla-server-api/src/oidc.rs` verifier to `crates/medulla-server-auth/src/auth.rs` to `dispatch_identity_and_access_methods` (`crates/medulla-server-router/src/dispatch/router/identity_access.rs:10`).
- `Method::Identity` to `dispatch_identity_store_method` (`identity_access.rs:90`) to `IdentityStore` (`crates/medulla-identity-types/src/identity/store.rs`).
- `Method::RbacAdmin` to the arm at `identity_access.rs:305`, which gains the share and ownership apply calls.
- `Method::Uql` and `Method::UnifiedQuery` to `handle_uql` (`crates/medulla-h-query/src/query/uql_statement.rs`) to `run_unified_off_lock_with` (`crates/medulla-h-query/src/query/planning.rs`), which gains the tenant predicate and the visible-unit filter.
- `Method::AgentAssemble` to `handle_agent_assemble` (`crates/medulla-h-decide/src/decide/assemble.rs:25`) to `assemble_for` (line 46), which gains `enforce_scope`.
- `Method::ApplyMutation` to `compile_methods` (`crates/medulla-server-api/src/batch_compile/compile.rs:124`), which gains `identity_metadata::admit`.
- Obsolete paths to delete: none; every row adds to an existing path.

## Cross-repo seams

- Identity mode transition and credential operations: `MEDULLA-IDENTITY-R003` and `MEDULLA-IDENTITY-R707` provide, `ARBORA-IDENTITY-R014` consumes. Contract: [identity-mode-and-credentials.md](contracts/identity-mode-and-credentials.md).
- Share grant and revoke: `MEDULLA-IDENTITY-R023.13` provides, `ARBORA-IDENTITY-R651` consumes. Contract: [share-grant-and-revoke.md](contracts/share-grant-and-revoke.md).
- Graph ownership plan and apply: `MEDULLA-IDENTITY-R700` provides, `ARBORA-IDENTITY-R652` consumes. Contract: [graph-ownership-grants.md](contracts/graph-ownership-grants.md).
- Session reference, liveness, approval keys and credential observation: `MEDULLA-IDENTITY-R708` to `MEDULLA-IDENTITY-R713` provide, `ARBORA-IDENTITY-R029.6` consumes. Contract: [session-and-approval-operations.md](contracts/session-and-approval-operations.md).
- Resource access decision and client session vocabulary: `MEDULLA-IDENTITY-R704` and `MEDULLA-IDENTITY-R705` provide, `ARBORA-IDENTITY-R019.3` and `ARBORA-IDENTITY-R026.2` consume. Contract: [resource-access-and-client-session.md](contracts/resource-access-and-client-session.md).
