# MEDULLA-DURABLE-KERNEL — Durable graph kernel

**Owner:** Medulla · **ID:** `durable-graph-kernel`

## Purpose and boundary

## Purpose and boundary

Medulla is the system of record for durable graph, table, KV, time series, outbox, audit, and transaction state. A caller may use native, RESP, SQL, or generated client surfaces, but equivalent operations must share one owner-store and policy path. The engine must recover without half-applied writes, prevent principal and tenant data leakage, preserve declared durability, and expose replay/repair evidence. Application orchestration and connector-specific business rules belong to their owning products. Attached-source adapters and virtual-query routing have their own [`unified-data-plane`](../unified-data-plane/spec.md) contract; this spec owns the native kernel and the transaction, durability, audit, replication, and mirror seams it uses.

Actors: application writer, read-only analyst, tenant administrator, replica/mirror consumer, and operator restoring a failed engine. Priority P0 is safe admission, atomic commit, recovery, and security. P1 is durable streaming and replication. P2 is latency and native storage expansion.

## User Scenarios & Testing

### User Story 1 - Commit a write exactly once, atomically (Priority: P1)

An application writer submits an authenticated MutationBatch and needs the commit to survive a restart exactly once, or to leave nothing behind.

**Independent Test**: Commit mixed-member batches, kill the engine at each fault point and reopen the store.

1. **Given** an authenticated MutationBatch for graph, table, KV and time-series members, **When** admission passes and the batch commits, **Then** the acknowledged result survives a restart exactly once (`MEDULLA-DURABLE-KERNEL-R043`).
2. **Given** a batch with an unsupported mixed member set, **When** it is submitted, **Then** it is refused before any effect and leaves no partial state (`MEDULLA-DURABLE-KERNEL-R034`).

### User Story 2 - Keep tenants and principals isolated (Priority: P1)

A tenant administrator needs one principal never to read or change another principal's private rows, sources, UDFs, caches or audit reservations.

**Independent Test**: Probe every read and write path with a foreign tenant and a foreign principal.

1. **Given** two principals in one tenant, **When** one names the other's private cache key or foreign source, **Then** the lookup fails closed with a stable access error (`MEDULLA-DURABLE-KERNEL-R018`).
2. **Given** a temporary elevation lease, **When** the proposer is also the approver or the digest differs, **Then** the engine refuses the grant (`MEDULLA-DURABLE-KERNEL-R022`).

### User Story 3 - Recover safely after a failure (Priority: P1)

An operator restoring a failed engine needs the store to validate its layout, replay committed state only and report corruption with typed findings.

**Independent Test**: Reopen stores with an old layout, an unsupported layout and injected corruption.

1. **Given** a store in an unsupported on-disk layout, **When** the engine opens it, **Then** it refuses with the named upgrade-required error and never reinterprets the layout (`MEDULLA-DURABLE-KERNEL-R003`).
2. **Given** a store with injected corruption, **When** the bounded scrub runs, **Then** it reports typed findings without blocking writers (`MEDULLA-DURABLE-KERNEL-R020`).

### User Story 4 - Stream and replicate committed changes (Priority: P2)

A replica or mirror consumer needs durable cursors and head-only attempt accounting, so one failing message cannot starve healthy ones.

**Independent Test**: Fail the head of a stream and replay it behind healthy successors.

1. **Given** a failing head row in an outbox stream, **When** the consumer retries, **Then** only the head gains attempts and successors keep a full budget (`MEDULLA-DURABLE-KERNEL-R004`).
2. **Given** a mirror target that was offline, **When** it catches up, **Then** it replays idempotently and compares source and target digests before it reports caught up (`MEDULLA-DURABLE-KERNEL-R036`).

### User Story 5 - Keep a multi-group cluster progressing (Priority: P2)

An operator needs independent leader progress per group and a committed batch observed exactly once across a leader failover.

**Independent Test**: Run the loaded cluster fixtures repeatedly on a clean host.

1. **Given** a multi-group cluster under load, **When** a leader fails, **Then** each group elects a leader independently and no committed batch is lost or repeated (`MEDULLA-DURABLE-KERNEL-R014`).
2. **Given** a failing cluster test, **When** it ends, **Then** a causal trace explains the heartbeat, append and recovery decisions without a manual toggle (`MEDULLA-DURABLE-KERNEL-R067`).

### User Story 6 - Serve point and structure operations natively and fast (Priority: P3)

An application writer needs declared durability classes, native point and structure routes, batched admission and a schema-versioned plan cache, with measured claims only.

**Independent Test**: Run the workload matrix and the kill-point tests per durability class.

1. **Given** a namespace declared `async`, **When** the engine is killed, **Then** the measured loss window stays within 100 ms (`MEDULLA-DURABLE-KERNEL-R038`).
2. **Given** a RESP or SQL batch with one failing member, **When** it executes, **Then** no earlier reply implies durability (`MEDULLA-DURABLE-KERNEL-R040`).

### Edge Cases

- A commit that fails after the first member must leave every member untouched (`MEDULLA-DURABLE-KERNEL-R034`).
- A foreign tenant naming a known key must get the same refusal as an unknown key (`MEDULLA-DURABLE-KERNEL-R018`).
- A store that opens read-only must refuse binding, because a bound scope is write authority (`MEDULLA-DURABLE-KERNEL-R006.1`).
- A lazy open that fails must not block the engine start (`MEDULLA-DURABLE-KERNEL-R006`).
- An outbox head that never succeeds must dead-letter without costing its successors an attempt (`MEDULLA-DURABLE-KERNEL-R004`).
- A mirror target that is not empty must refuse to start (`MEDULLA-DURABLE-KERNEL-R024`).
- A downgrade of a namespace durability class must be refused (`MEDULLA-DURABLE-KERNEL-R038`).

## Requirements

- **FR-01**: MutationBatch is the sole persistent write envelope. All store members use the owner manifest, one admission decision and one idempotent terminal outcome. Rows: MEDULLA-DURABLE-KERNEL-R043, MEDULLA-DURABLE-KERNEL-R012, MEDULLA-DURABLE-KERNEL-R023. Proof: restart/replay and mixed-member tests on a main revision.
- **FR-02**: Transaction commit is atomic across all participating members, or explicitly refuses an unsupported mixed transaction before effects. Table-only transactions never enter the mixed-store path. A chosen cross-store mechanism has an ADR and fault-point proof. Rows: MEDULLA-DURABLE-KERNEL-R034. Proof: crash matrix and concurrent anomaly corpus.
- **FR-03**: Owner manifest validation occurs at manifest read and at the write transaction boundary. Cache a validated binding only within that transaction. Format changes have versioned lineage and typed upgrade refusal. Rows: MEDULLA-DURABLE-KERNEL-R003, MEDULLA-DURABLE-KERNEL-R021. Proof: lineage golden fixtures and stale-binding negative tests.
- **FR-04**: A bounded, resumable, read-only content scrub reports typed corruption findings without blocking writers or treating per-commit graph unseal as a scrub. Rows: MEDULLA-DURABLE-KERNEL-R020. Proof: injected corruption/restart/rate-limit tests.
- **FR-05**: The outbox increments an attempt only for the attempted head, supports consumer reject and operator rewind, and never acknowledges a skipped reasoning projection without a rebuild. Rows: MEDULLA-DURABLE-KERNEL-R001, MEDULLA-DURABLE-KERNEL-R002, MEDULLA-DURABLE-KERNEL-R004, MEDULLA-DURABLE-KERNEL-R005. Proof: failing-head and successor replay tests.
- **FR-06**: CDC and mirrors publish only committed outcomes, use stable per-consumer cursors, replay idempotently after outage, and compare source and target digests before declaring caught up. Mirror reads cannot become an alternate authority. Rows: MEDULLA-DURABLE-KERNEL-R024, MEDULLA-DURABLE-KERNEL-R036. Proof: outage, replay, drift and empty-target tests.
- **FR-07**: Every read/write path carries verified tenant and principal scope through RLS, SQL, UQL, TSDB, KV, foreign sources, UDFs and caches. Cross-scope lookup fails closed; sharing requires an explicit grant. Foreign-source sharing is specified by `federated-query-and-reasoning`; app consolidation policy is specified by `unified-data-plane`. Rows: MEDULLA-DURABLE-KERNEL-R018, MEDULLA-DURABLE-KERNEL-R019, MEDULLA-DURABLE-KERNEL-R029. Proof: served cross-principal and cross-tenant denial matrix.
- **FR-08**: Governed effects reserve a tenant-scoped idempotent audit record before the effect and close it with linked outcome. An unavailable writer or missing audit class blocks the effect, and interrupted outcomes reconcile to a truthful state. Rows: MEDULLA-DURABLE-KERNEL-R031. Proof: duplicate request, outage, crash and mismatch tests.
- **FR-09**: Temporary elevation and schema repair use separate reserved lease kinds with two distinct verified principals, exact request digest, expiry and audit; generic lease write cannot grant either. Rows: MEDULLA-DURABLE-KERNEL-R022, MEDULLA-DURABLE-KERNEL-R027. Proof: proposer=approver, wrong digest, expired and replay refusals.
- **FR-10**: Raft dispatch preserves per-peer and per-group progress. Multi-node tests capture causal traces on failure and repeatedly prove leader failover and multi-group writes without sleep/retry/assertion relaxation. Rows: MEDULLA-DURABLE-KERNEL-R008, MEDULLA-DURABLE-KERNEL-R009, MEDULLA-DURABLE-KERNEL-R013, MEDULLA-DURABLE-KERNEL-R014, MEDULLA-DURABLE-KERNEL-R015, MEDULLA-DURABLE-KERNEL-R016, MEDULLA-DURABLE-KERNEL-R025. Proof: repeated loaded cluster tests and trace artifact.
- **FR-11**: Namespace/table durability is declared, queryable and never downgraded: `sync` commit-before-ack default; `async` measured loss window ≤100 ms; `ephemeral` RAM+TTL and lost on restart. Authz and principal-bound keyspace apply to all classes. Rows: MEDULLA-DURABLE-KERNEL-R038. Proof: kill-point and measured-loss tests.
- **FR-12**: In-memory hash, sorted set, list and set structures are working state only. The redb log/recovery rules remain the durable authority for async/sync. Native atomic primitives provide `INCRBY`+expiry, `SET NX EX`, CAS lease, token bucket, bounded counter and work claim. Rows: MEDULLA-DURABLE-KERNEL-R039. Proof: concurrent linearizability/rebuild tests.
- **FR-13**: RESP and SQL extended-protocol batches receive one admission and one group commit with ordered N replies. A failed member cannot yield earlier success replies implying durability. Rows: MEDULLA-DURABLE-KERNEL-R040. Proof: partial failure and response-order tests.
- **FR-14**: The native route handles point/structure requests without a DataFusion session. SQL plans are keyed by statement digest and schema version and invalidated with the existing dependency clock. Rows: MEDULLA-DURABLE-KERNEL-R037, MEDULLA-DURABLE-KERNEL-R041. Proof: route and invalidation tests.
- **FR-15**: Native row hot/cold union, store-engine choice, replication-out and operational compatibility are admitted only with differential, restore and workload evidence; no mirror or external cache becomes a read authority. Rows: MEDULLA-DURABLE-KERNEL-R032, MEDULLA-DURABLE-KERNEL-R033, MEDULLA-DURABLE-KERNEL-R035, MEDULLA-DURABLE-KERNEL-R042. Proof: ADR, corpus, restore drill and benchmark report.
- **FR-016**: Each lifecycle named in these rows is an explicit state machine with one transition function, one stored state field, a typed refusal of illegal transitions and an ontology description (organization constitution Article XXXI). Rows: `MEDULLA-DURABLE-KERNEL-R872`, `MEDULLA-DURABLE-KERNEL-R872.1`, `MEDULLA-DURABLE-KERNEL-R872.2`.

## Success Criteria

- **SC-01**: An acknowledged sync commit is present exactly once after every kill point of the crash matrix (`MEDULLA-DURABLE-KERNEL-R034`).
- **SC-02**: Every cross-scope lookup in the isolation corpus returns a stable refusal and leaks no row (`MEDULLA-DURABLE-KERNEL-R018`).
- **SC-03**: A store with an unsupported layout is refused in 100 percent of the lineage fixtures (`MEDULLA-DURABLE-KERNEL-R003`).
- **SC-04**: A failing outbox head costs its successors zero attempts in every replay fixture (`MEDULLA-DURABLE-KERNEL-R004`).
- **SC-05**: The loaded cluster fixtures pass in every repeated run with no sleep, retry or relaxed assertion (`MEDULLA-DURABLE-KERNEL-R014`).
- **SC-06**: Every published speed claim names its workload, class, hardware and measured p99 (`MEDULLA-DURABLE-KERNEL-R042`).

## Assumptions

- The engine is the only fact store; mirrors and caches never become read authority.
- Raft, redb, the MutationBatch envelope and the owner manifest stay the base of the kernel, and rows reuse them.
- Hosted gates run on an ephemeral runner with declared fixtures; a live private service is optional deployment proof.
- Owner decisions that remain open are named in the row that needs them and are not assumed here.
- Arbora, the harness and the web client consume this kernel through the served contracts of their own specs.

## Scope and dependencies

`FR-01`–`FR-10` consolidate kernel obligations that this spec covers directly; `FR-11`–`FR-15` are queued native-storage work not yet accepted. Serving clients and finance workloads exercise this kernel but do not move their orchestration or accounting into it. The attached-source registry, CDC adapters, dialects and query federation remain in `unified-data-plane`; both specs use the same ChangeEnvelope, position and audit contracts. Owner-specific app cutover is never implied by this spec.

For every implementation PR, link each changed `FR-` label and row, and land the `Spec:` trailer with the bound test. State is computed from landings and bound tests, never written. A branch commit alone is not acceptance; the matching positive and negative tests at the merged head are.

### Detailed migration crosswalk

This table makes the historical work identifiers searchable without requiring their drafts. It records the present migration classification, not an acceptance verdict. Where an old item was a specific gate repair, the durable requirement below is the continuing contract.

| IDs | Current classification | Normative destination here |
|---|---|---|
| MEDULLA-DURABLE-KERNEL-R043. MEDULLA-DURABLE-KERNEL-R001, MEDULLA-DURABLE-KERNEL-R002, MEDULLA-DURABLE-KERNEL-R003, MEDULLA-DURABLE-KERNEL-R004, MEDULLA-DURABLE-KERNEL-R005, MEDULLA-DURABLE-KERNEL-R006, MEDULLA-DURABLE-KERNEL-R007 | source landed or built. Acceptance unverified | FR-01, FR-03, FR-05. Owner layout, outbox and local delivery state |
| MEDULLA-DURABLE-KERNEL-R008, MEDULLA-DURABLE-KERNEL-R009, MEDULLA-DURABLE-KERNEL-R012, MEDULLA-DURABLE-KERNEL-R013, MEDULLA-DURABLE-KERNEL-R014, MEDULLA-DURABLE-KERNEL-R015, MEDULLA-DURABLE-KERNEL-R016, MEDULLA-DURABLE-KERNEL-R025 | source landed or built. Loaded repeat proof open | FR-01, FR-10. Raft command, leader forwarding and durability |
| MEDULLA-DURABLE-KERNEL-R020, MEDULLA-DURABLE-KERNEL-R021, MEDULLA-DURABLE-KERNEL-R017 | built; acceptance unverified | FR-03, FR-04, FR-15; scrub, binding and correctness benchmark |
| MEDULLA-DURABLE-KERNEL-R018, MEDULLA-DURABLE-KERNEL-R019, MEDULLA-DURABLE-KERNEL-R022, MEDULLA-DURABLE-KERNEL-R026, MEDULLA-DURABLE-KERNEL-R027, MEDULLA-DURABLE-KERNEL-R028, MEDULLA-DURABLE-KERNEL-R029, MEDULLA-DURABLE-KERNEL-R030 | landed, built or queued by item. Acceptance unverified | FR-07, FR-09. Scope, auth mode, RLS and lease denial |
| MEDULLA-DURABLE-KERNEL-R023, MEDULLA-DURABLE-KERNEL-R031, MEDULLA-DURABLE-KERNEL-R034, MEDULLA-DURABLE-KERNEL-R035, MEDULLA-DURABLE-KERNEL-R036 | built or queued. Acceptance unverified | FR-02, FR-06, FR-08, FR-15. Readonly operation classification, atomicity, audit, mirrors and tooling |
| MEDULLA-DURABLE-KERNEL-R037, MEDULLA-DURABLE-KERNEL-R038, MEDULLA-DURABLE-KERNEL-R039, MEDULLA-DURABLE-KERNEL-R040, MEDULLA-DURABLE-KERNEL-R041, MEDULLA-DURABLE-KERNEL-R042 | queued | FR-11–FR-15; native routing, durability, hot state, batch and measurement |
| MEDULLA-DURABLE-KERNEL-R010, MEDULLA-DURABLE-KERNEL-R011 | gate source landed or deferred | Quality proof: differential clones and feature-specific Rust coverage; no runtime feature claimed |

The method-codec artifact MEDULLA-CONTRACT-R006 belongs to the [public engine contract and release spec](../public-engine-contract-and-release/spec.md). It does not define a second security mechanism here. Pack-specific manifest refusal MEDULLA-TYPED-PACKS-R056 belongs to [typed packs and catalog](../typed-packs-and-catalog/spec.md). User-managed index lifecycle MEDULLA-FEDERATED-QUERY-R009 and foreign-source grants MEDULLA-FEDERATED-QUERY-R013/MEDULLA-FEDERATED-QUERY-R014 belong to [federated query and reasoning](../federated-query-and-reasoning/spec.md). Attached-source acceleration MEDULLA-UNIFIED-DATA-PLANE-R010 and app isolation MEDULLA-UNIFIED-DATA-PLANE-R031 belong to [unified data plane](../unified-data-plane/spec.md). These references are integration boundaries, not duplicate owner claims.

## Quality and exclusion rules

Reuse `crates/medulla-types` MutationBatch, `crates/medulla-transaction` admission, `crates/medulla-storage` owner manifest, existing redb and Raft stores, `src/server` auth/wires and `medulla-tsdb`/`medulla-stream`. No parallel persistent log store, cache product, alternate write authority, or unguarded fast path. Apply the repository's configured CCCC changed-function gate (no new function over cyclomatic 10 or cognitive 15), Dupehound structural clone gate, jscpd differential gate, KISS changed-Rust gate, Rust architecture lint, formatting, clippy and relevant tests. The CI gate must be runnable on an ephemeral cloud runner with declared fixture provisioning; live private services are an optional deployment proof, never the sole code acceptance route.

Requirement IDs are defined in [requirements.md](requirements.md); state is computed from landings and bound tests.

## Worker registry producer boundary

R074.1 owns the pure typed capacity and liveness transitions in medulla-types. R074.2 owns authenticated served dispatch, ownership enforcement and generated clients. R074 remains open until both children satisfy their tests; the producer alone does not serve a registry.

## Scheduler config producer boundary

R076 is split into R076.1 and R076.2. The first owns the pure typed policy and validation in `medulla-types`. The second must expose and consume it through the real engine setting/admission paths. Model tests do not establish a served setting or complete R076. MEDULLA-PLACEMENT-R002.2 extends the producer contract with autosize defaults and queue depth.

## Required runtime manifest boundary

R011.2 owns the concrete crate, feature, and module-filter declarations in `medulla-types`.
The declaration refuses an empty discovered selection.
R011.3 owns pre-merge execution; R011.4 owns exact-head CI acceptance receipts.
Manifest unit tests do not establish runtime acceptance for R011.

## Safe eviction and durable read-through boundary

MEDULLA-DURABLE-KERNEL-R086 records the existing node-residency and durable point-read contract.
It adds explicit cache-miss, scope, refusal and restart evidence under this durability owner.
R039 rebuild acceptance and all existing receipts remain unchanged.
A registry reopen, resident read or declared seam is insufficient proof of safe eviction.
This definition batch changes no runtime behavior and claims no completed R086 implementation.
Lifecycle state is computed, never written here.

## Durable-kernel owner definitions and explicit holds

These rows define only the Medulla-owned storage and refusal boundary. Meristem and Arbora behavior remains a dependency where named. No implementation or served receipt is claimed.

### `MEDULLA-DURABLE-KERNEL-R028.1`

A run checkpoint is stored as an immutable R085 `active_context` memory snapshot. Its canonical UTF-8 body binds `run_id`, `work_item_id`, current lease epoch/fencing token, checkpoint revision and payload digest. It also binds separate short-term and long-term memory document references by document ID, revision and digest. Tenant and graph come only from the verified request context. A checkpoint update must be fenced against the live WorkItem lease epoch and fencing token. Current Medulla surfaces lack an atomic operation joining an R085 snapshot update to the WorkItem checkpoint pointer/fence. `CasWorkItemMetadata` is scheduling metadata, not a checkpoint transaction. The transaction/recovery contract remains held. `SyncMemorySnapshot` uses the existing MutationBatch path and expected-head comparison; `ReadMemorySnapshot` returns the accepted revision and digest. R039 remains the sole redb durability authority. Stale revision/fence, cross-tenant or cross-graph reference, or digest mismatch refuses without advancing the head. Changed-body replay identity or unavailable durable storage also refuses without advancing the head.

Medulla does not perform resume or stream operations. Open decisions include retention bounds and release authority for the two memory classes. Meristem must define the exact resume request/receipt binding to the checkpoint and current WorkItem fence. The host owner must define stream ID/cursor authority and replay semantics. Keep the original `MEDULLA-DURABLE-KERNEL-R028`, `MEDULLA-IDENTITY-R006`, `ARBORA-A2A-R001`, `ARBORA-A2A-R001.1`, and `ARBORA-OPS-R001` obligations.

### `MEDULLA-DURABLE-KERNEL-R085.9`

Represent search state in an R085 `active_context` snapshot keyed by stable search-run document ID. Required exact UTF-8 JSON fields are nonempty-string `search_run_id`, positive `state_revision` equal to the snapshot revision, and array-valued `transcript`, `evidence`, `constraints`, and `open_claims` (arrays may be empty). Medulla validates this envelope and hashes exact bytes; Meristem owns array-item schemas and source identities. R085 provides graph-incarnation binding, exact content digest, immutable revision, expected-head compare-and-set and conflict retention. It also covers authorization, a one-MiB content ceiling, retention holders and durable MutationBatch write. Meristem must write the complete state before context eviction and resume from the exact document ID/revision returned by `ReadMemorySnapshot`. Medulla returns stored state and does not decide how Meristem compacts or handles it. Missing/malformed required fields, oversize body, wrong authority, stale graph, digest mismatch, or storage failure refuses without head advance. A stale expected revision retains a conflict under R085 semantics. Preserve `MERISTEM-HARNESS-R013.13`, `MERISTEM-HARNESS-R020.1`, `MEDULLA-DURABLE-KERNEL-R085.1`, `.4`, and `.7`.

Open Meristem decisions include per-entry schemas and source identity for each of the four fields. Meristem must also define the continuation request/receipt field binding resume to this exact memory revision. No Meristem continuation or context-eviction success is claimed by this Medulla contract.

### `MEDULLA-DURABLE-KERNEL-R085.10`

Persist prior-session notes only as R085 versioned memory snapshots. Bind stable note ID, source run/task references, captured source revision, canonical UTF-8 content/digest and provenance. Also bind graph incarnation and parent/current revision. Use only `SyncMemorySnapshot`, `ReadMemorySnapshot`, and `ListMemorySnapshots` through the existing authorized gateway and R039 authority. Medulla returns the currently authorized revision and explicit R085 sync/conflict state. Stale submissions retain the candidate without changing the accepted head. Medulla never injects notes into a prompt or selects relevance. Preserve `MERISTEM-HARNESS-R022.3`, `MEDULLA-DURABLE-KERNEL-R085.2`, `.5`, and `.7`.

Open Meristem decisions: the capture trigger, note-kind mapping, relevance score and threshold, staleness rule, and presentation/selection of conflicting notes. Automatic capture and transparent contextual reuse remain held until Meristem defines those inputs and consumer behavior.


## Final mapped contract extensions

These definitions are planned contracts. They do not claim implementation or served evidence.

### `MEDULLA-DURABLE-KERNEL-R038.100`

Define an explicit deployment profile with `embedded` (in-process operation requiring no external service) and `served` (configured service endpoint) modes. Profile selection controls transport/startup only. It does not silently change the existing persistent-namespace durability default or sync/redb acknowledgment authority. Ephemeral in-memory namespaces require an explicit `ephemeral` declaration and retain no durable log. Persistent namespaces retain their declared `sync` or `async` class. Do not adopt the source request’s universal in-memory default.

**Producer/consumer boundary:** Medulla owns profile and storage selection. Transport mode is separate from namespace DurabilityClass. Preserve R038 durability classes, sync default, redb authority and authorization/HMAC isolation.

**Preserved parent:** `MEDULLA-DURABLE-KERNEL-R038` remains unchanged.


## Cross-owner committed change feed

`ApplyChangeEnvelope` remains the sole commit authority. The proposed `OpenGraphSnapshotV1` returns a graph-incarnation/revision snapshot digest and an exact high-water cursor from the same read. `ReadGraphChangesV1` returns ordered commit/event-ordinal records, including authorized tombstones, using an opaque cursor bound to tenant, graph and authorization revision. It provides stable ids for replay deduplication, a 1..256 page bound, 24-hour retention, `CURSOR_EXPIRED` recovery and `AUTHORIZATION_CHANGED` refusal. Every delivered event exposes versioned commit-to-visible timing evidence. Served p95 target is ≤1000 ms from commit acknowledgement to first authorized read.

The feed is read-only over durable commits, not an alternative write envelope or a local browser echo. Arbora routes the operations to Overstory R030.8. The paired two-client test verifies reconnect/order and independent graph readback. The consumer's readable layout remains a separate Overstory obligation.

## Reviewed benchmark measurement extension — `MEDULLA-DURABLE-KERNEL-R042.100`

The existing `BenchmarkClaim.measured_result` gains typed `BenchmarkMeasurementV1`. It binds benchmark configuration/workload, dataset/query digest, release commit, model/provider/tokenizer revisions and hardware/profile settings. It also binds model-ready and request timestamps, complete input-token count, inference elapsed time and pinned price table. Throughput uses all input tokens divided by post-ready inference time. Cost is calculated per query from the pinned prices and currency exponent with a specified rounding rule. Startup is reported but never included in inference time or throughput. Only identical workload/profile measurements are comparable. Missing tokenizer/token count/price, invalid elapsed time, zero denominator or mismatched profiles refuse claim admission. This extends the existing claim-validation record and does not introduce another benchmark runner. It remains SPECIFIED. No performance result is asserted.
