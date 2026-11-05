# MEDULLA-REPO-INGEST — Repository index and native ingestion

Component: Repository index and ingestion

## Outcome

A contributor can index a repository snapshot into a durable, queryable graph without an application-specific graph writer. A caller supplies authenticated, immutable source bytes and revision membership; the engine determines parse outcomes, resolves symbols across files, records provenance, commits the projection once, and reports enough evidence to retry failures. A query can find the exact source occurrence and revision behind an answer. This contract also defines the Medulla side of general source ingestion; it does not require access to any private workspace or service.

## Scope

Owner: `medulla` (native Rust). Medulla owns `Method::IndexRepository`, `IndexRepositoryScope`, `IndexResult`, `SourceIngest`, `ChangeEnvelope`, graph projection, CAS binding, native parsing/resolution, admission, durable work and queryable lineage. The connector connector SDK owns provider authentication, immutable snapshot fetch, paging, checksum validation and transport; it may retry based on Medulla's typed outcomes but cannot mint graph facts or checkpoint authority. Agent applications may request ingestion and consume receipts but cannot bypass Medulla's projection. Repository tooling may discover repositories and provide authenticated source coordinates, but is not a second graph writer.

This feature does not require a hosted Git provider, private manifest, local service inventory, model service or running cluster for its deterministic development tests. Served multi-node and provider integration remain separate release evidence.

Non-goals: no coverage of independent engine work such as layout upgrades, community detection, domain-specific connectors, query grammar, or unrelated quality regressions. Those concerns have their own owner specs.

Why a new spec: repository indexing and the Medulla side of source ingestion form one owned capability. The search `git grep -n "IndexRepository\|SourceIngest" -- specs` found no other owner. Foreign-source scope belongs to `federated-query-and-reasoning`, and isolation belongs to `durable-graph-kernel`.

## User Scenarios & Testing

### User Story 1 - Index a repository snapshot atomically (Priority: P1)

The user is a repository contributor. The outcome is a durable graph of branches, files, symbols and edges that commits once and replays safely.

**Acceptance scenarios**

1. **Given** two refs that share one blob, **When** the snapshot is indexed, **Then** the blob parses once and each ref keeps its own membership.
2. **Given** a batch above the commit budget, **When** it is submitted, **Then** the engine refuses it whole with `REPOSITORY_BATCH_TOO_LARGE` and commits nothing.
3. **Given** two relations between the same two entities, **When** the batch lowers, **Then** the graph holds two distinct typed edges.

Independent test: the bound tests of `MEDULLA-REPO-INGEST-R008` run this story through the served harness.

### User Story 2 - Understand code after the native commit (Priority: P1)

The user is a code intelligence consumer. The outcome is typed code, test, history and link facts that never delay the index response.

**Acceptance scenarios**

1. **Given** a committed snapshot, **When** post-commit enrichment runs, **Then** typed entities and rung-ranked edges commit in a second envelope.
2. **Given** an edge that would overwrite a higher-rung edge, **When** enrichment applies it, **Then** the engine refuses and counts it.
3. **Given** a repository with no declared entry point, **When** the code-health sweep runs, **Then** it reports `unknown` and never `dead`.

Independent test: the bound tests of `MEDULLA-REPO-INGEST-R600.4` and `MEDULLA-REPO-INGEST-R700.4` cover the refusals.

### User Story 3 - Ingest governed source observations (Priority: P1)

The user is a source operator. The outcome is a checkpointed, replayable ingestion with stable refusal codes.

**Acceptance scenarios**

1. **Given** an envelope applied twice, **When** the second apply arrives, **Then** the engine returns the first commit and writes nothing.
2. **Given** a stranded semantic intent, **When** a reconcile pass runs, **Then** the intent is re-admitted.
3. **Given** a replayed request nonce, **When** it arrives after a restart, **Then** the engine refuses it.

Independent test: the bound test of `MEDULLA-REPO-INGEST-R001` covers reconcile; `MEDULLA-REPO-INGEST-R005` covers replay.

### User Story 4 - Hydrate stores and ingest documents and sessions (Priority: P2)

The user is an operator. The outcome is an empty store that fills from configured sources, with documents chunked into verified section trees and agent sessions ingested as records.

**Acceptance scenarios**

1. **Given** an empty store and configured sources, **When** the server boots, **Then** it plans baseline hydration and submits jobs, and a second boot skips unchanged content.
2. **Given** a document, **When** it is enriched, **Then** the section tree nests, covers the text and is stored with a processed-document record.
3. **Given** a transcript whose prefix changed, **When** intake runs, **Then** the session restarts at the first differing ordinal.

Independent test: `crates/medulla-document/src/runtime.rs::transcript_prefix_delta_restarts_at_first_changed_or_removed_line`.

## Edge Cases

- A tombstone removes only the named ref, path and blob membership.
- A parser upgrade reparses an earlier unsupported or error blob without duplicating a successful revision.
- A batch whose stated base revision differs refuses before any write.
- A file that fails to parse is classed `Structural` and is never skipped.
- A stale code-map generation refuses every page and cursor.

## Assumptions

- The connector SDK sends authenticated, immutable source bytes and revision membership.
- Enrichment may fail without rolling back an accepted native snapshot.
- The native commit stays within the 64 MiB durable ledger record budget.
- Every new served method registers a descriptor in the served catalog.

## Requirements

### Functional requirements

The table below states the behaviour. Each row of [requirements.md](requirements.md) binds it to code and proof.

| ID | Requirement and acceptance rule |
|---|---|
| RI-01 | `IndexRepository` accepts a graph-scoped `IndexRepositoryScope` with repository ID, immutable Git commit IDs, ref status, file-version memberships and tombstones. It rejects malformed logical paths, invalid digests, duplicate or contradictory membership, and a ref that claims a mutable revision. The authenticated tenant and graph authorization come from the request boundary, never request fields. |
| RI-02 | One unique blob is parsed once per batch; the same bytes can belong to many paths and refs. `Blob`, `FileVersion`, and `Branch` graph entities preserve both content and occurrence identity. Declaration IDs distinguish identical text at different paths/offsets. Repeated input order or hash-map iteration does not change canonical IDs or the commit digest. |
| RI-03 | Every submitted blob returns one ordered `IndexFileOutcome` with `success`, `unsupported`, or `error`, canonical content and parser-capability digests, and bounded machine-readable diagnostics. Empty but valid source is `success`; unsupported language is never silently treated as empty success. Failed or unsupported blobs remain eligible for later reparse when parser capability changes. |
| RI-04 | Cross-file resolution uses a deterministic symbol table and emits only edges whose endpoints exist in the batch or admitted graph. Ambiguous imports/calls remain unresolved with counts and diagnostics; no guessed `depends_on` or `similar_to` edge may be committed. Model-free similarity carries algorithm/version and score, and must be repeatable for identical input. |
| RI-05 | A scoped batch lowers to one governed `ChangeEnvelope` and one durable graph transaction, including graph projection, source references, privacy policy, provenance, tombstones, and enrichment intent. Identical replay returns the original committed result without graph-version advancement. A crash before the commit can leave CAS content but cannot create partial graph rows or a success receipt. |
| RI-06 | A tombstone removes only the named ref/path/blob membership. A rename records old membership removal and new membership addition; shared blob and other refs remain valid. Deleting a ref removes its live membership without erasing immutable historical revisions. |
| RI-07 | Repository source bytes and source-relative paths are scoped to the verified tenant and repository. Graph reads and writes enforce graph ACL. Raw host identity is redacted before durable projection; code punctuation and source-relative paths are preserved. Untrusted source cannot supply policy, graph mutations, ownership, or tenant. |
| RI-08 | An over-limit batch is refused atomically with `REPOSITORY_BATCH_TOO_LARGE` before any graph mutation. The response exposes enough bounded detail for a caller to halve and resend. The same code covers operation and durable-record byte/item ceilings; limit changes are versioned in the generated contract. |
| RI-09 | Native rungs 0–2 (parse/AST, cross-file resolution, model-free statistics) finish without an external model call. Rungs 3–5 are represented as durable, budgeted work items with admission, lease, retry/park, completion and provenance. Underfunded or unauthorized work fails closed. Query-triggered enrichment is bounded and does not mutate an earlier revision. |
| RI-10 | A canonical semantic bridge maps repository entities into the engine graph. It also maps their source, test, spec and release relationships without duplicate writer authority. Each edge has source evidence and a stable schema/version. A missing target becomes an unresolved relation, not an invented entity. |
| RI-11 | `SourceIngest` accepts typed observations with manifest references, raw evidence, exact expected checkpoint, and mode (`full`, `delta`, `reconcile`). Medulla resolves approved mappings and commits graph changes, live-set marker, tombstones, lineage, checkpoint and receipt atomically. Empty authoritative reconciliation requires the explicit approval and capability. `SourceIngestStatus` reads the durable checkpoint after restart. |
| RI-12 | Change consumers apply source-position-idempotent graph/index/outbox updates. A Debezium-style change bridge maps an ordered source event into `ChangeEnvelope`, preserves source position and deletion semantics, and refuses unknown schema or out-of-order replay. Any accelerated read declares freshness; read-your-writes waits for the caller's committed position within a bounded timeout or reports stale. |

The labelled requirements bind the table to rows:

- FR-001 Repository batches lower to one governed envelope and commit once (`MEDULLA-REPO-INGEST-R009.2`, `MEDULLA-REPO-INGEST-R008`).
- FR-002 Over-limit batches refuse whole with a stable code (`MEDULLA-REPO-INGEST-R007`, `MEDULLA-REPO-INGEST-R008`).
- FR-003 Edge identity is the triple of source, target and relation type (`MEDULLA-REPO-INGEST-R009.1`).
- FR-004 Heavy enrichment never blocks the native commit (`MEDULLA-REPO-INGEST-R006`, `MEDULLA-REPO-INGEST-R600.3`).
- FR-005 Source ingestion applies envelopes idempotently with typed refusals (`MEDULLA-REPO-INGEST-R002.651.1`, `MEDULLA-REPO-INGEST-R002.653`).
- FR-006 Documents, sessions and baseline hydration reuse the same writer (`MEDULLA-REPO-INGEST-R652.3`, `MEDULLA-REPO-INGEST-R800.3`, `MEDULLA-REPO-INGEST-R650.3`).

### Success criteria

- SC-001 A two-ref fixture with a shared blob gives exact membership and one parse per unique blob (`MEDULLA-REPO-INGEST-R009.2`).
- SC-002 The native commit p99 stays at most 5 s on 5,000 files (`MEDULLA-REPO-INGEST-R006`).
- SC-003 Concurrent, stale, oversized and interrupted writes fail with stable codes and leave the graph version unchanged (`MEDULLA-REPO-INGEST-R008`).
- SC-004 A code query reaches specification, test and release edges without another store (`MEDULLA-REPO-INGEST-R009`).

## Traceability

| Requirement | Stable IDs | Current evidence | Required acceptance proof |
|---|---|---|---|
| RI-01–RI-06 | MEDULLA-REPO-INGEST-R009, MEDULLA-REPO-INGEST-R006 | Branch-aware types, parser, durable index tests exist | Exact-head public API, reparse, identity and restart tests |
| RI-07–RI-08 | MEDULLA-REPO-INGEST-R007, MEDULLA-REPO-INGEST-R008 | Tenant and budget guards exist in parts | Cross-principal refusal, large-batch and generated-client contract |
| RI-09 | MEDULLA-REPO-INGEST-R006, MEDULLA-REPO-INGEST-R004, MEDULLA-REPO-INGEST-R005 | Native rung evidence and an enrichment consumer exist | Durable admission/lease/restart, bounded query and throughput receipt |
| RI-10 | MEDULLA-REPO-INGEST-R009, MEDULLA-REPO-INGEST-R002, MEDULLA-REPO-INGEST-R003 | Repository graph projection exists | Semantic bridge with complete source/spec/test/release chain |
| RI-11 | MEDULLA-REPO-INGEST-R001, MEDULLA-REPO-INGEST-R002 | Native `SourceIngest` exists | Mapping, checkpoint, replay and fresh-store tests |
| RI-12 | Shared interface; owned by `unified-data-plane` | Source-position design required | Idempotent consumer, freshness and routing tests |

Foreign-source scope MEDULLA-FEDERATED-QUERY-R013 belongs to `federated-query-and-reasoning`; UDF/lifecycle isolation MEDULLA-DURABLE-KERNEL-R018/MEDULLA-DURABLE-KERNEL-R019 belongs to `durable-graph-kernel`. The source change consumers, freshness router and Debezium bridge MEDULLA-UNIFIED-DATA-PLANE-R009/MEDULLA-UNIFIED-DATA-PLANE-R011/MEDULLA-UNIFIED-DATA-PLANE-R021 belong to `unified-data-plane`. This spec uses those contracts for repository indexing without claiming their implementation authority.

Requirement IDs are in [requirements.md](requirements.md). Delivery state per ID is in `status.json`.
