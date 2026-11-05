# MEDULLA-OBJECT-STORAGE-BACKEND — Object-storage durable store and comparative cost gate

Component: Object-storage backend

## Outcome

A graph's durable state can live in an S3-compatible object store, instead of the local redb store or beside it. Any node opens any tenant graph from the shared root. Commits are commit-before-ack over immutable versioned segments. Every storage operation has a measured cost across releases, and a comparative gate carries the numbers. The local store remains the default. Object storage is a declared, typed backend and not a second engine.

The object-store backend stays under the existing multi-raft consensus, placement and resharding authorities. Its comparative cost gate also counts cross-shard federation actions, which a single-store harness cannot express.

## Scope

Owner: `medulla` (native Rust), sole owner. The code lands in `crates/medulla-storage` (backend spec, object layout, fetch, archive, counting wrapper), `crates/medulla-persistence` (`ObjectStoreBackend`), `crates/medulla-persistence-ops` and `src/operator_command.rs` (graph copy), `crates/medulla-server` (bootstrap selection) and the nightly bench harness. Non-goals: no second consensus layer, no second S3 client, no second layout registry and no writer authority beyond Raft. Arbora and the deployment own where the setting is supplied.

Why a spec: no other spec owns the object-store segment layout, the manifest publish, or the cost gate.

## User Scenarios & Testing

### User Story 1 - Declare a backend and be refused before any effect (Priority: P1)

The user is an operator who picks a durable-store backend. The outcome is an admitted spec or a typed refusal, so a mis-declared store never opens.

**Acceptance scenarios**

1. **Given** the setting names `local` or nothing, **When** a shard opens, **Then** the redb store opens as before.
2. **Given** an `object_store` spec with no secret reference, no durability class or an unknown layout digest, **When** the engine admits it, **Then** it refuses with the typed reason and changes no state.

Independent test: the bound tests of `MEDULLA-OBJECT-STORAGE-BACKEND-R001`.

### User Story 2 - Open a graph cold from the object root (Priority: P1)

The user is an operator whose node lost its local data. The outcome is a graph restored from the root with every acknowledged write.

**Acceptance scenarios**

1. **Given** an engine wrote 100 nodes and stopped, **When** the local directory is deleted and a second engine opens the same root, **Then** it reads all 100 nodes.
2. **Given** a segment is corrupted, **When** the engine opens the version, **Then** it refuses with `OBJECT_SEGMENT_DIGEST_MISMATCH` naming the version and key, and earlier versions stay readable.

Independent test: the bound tests of `MEDULLA-OBJECT-STORAGE-BACKEND-R002` and `MEDULLA-OBJECT-STORAGE-BACKEND-R002.1`.

### User Story 3 - Commit before ack, and copy a redb graph to the root (Priority: P1)

The user is a writer, or an operator who migrates a graph. The outcome is an ack only after a durable head update, and a verified copy.

**Acceptance scenarios**

1. **Given** the engine dies between the segment upload and the `HEAD` update, **When** it restarts, **Then** acked writes are present and the unacked write is absent.
2. **Given** two publishers race on one version, **When** both put `HEAD`, **Then** exactly one wins and the loser refuses with `OBJECT_PUBLISH_STALE`.
3. **Given** a graph copy batch is corrupted, **When** the copy verifies the digest, **Then** it refuses with `GRAPH_COPY_DIGEST_MISMATCH` and the destination stays empty.

Independent test: the bound tests of `MEDULLA-OBJECT-STORAGE-BACKEND-R003`, `MEDULLA-OBJECT-STORAGE-BACKEND-R003.1` and `MEDULLA-OBJECT-STORAGE-BACKEND-R003.2`.

### User Story 4 - Open the same root from several nodes (Priority: P2)

The user is an operator running several nodes. The outcome is identical served state on each node with no second consensus layer.

**Acceptance scenarios**

1. **Given** two nodes with a placement route open one root, **When** both serve a read, **Then** the state is identical.
2. **Given** a node holds no placement route for the tenant, **When** it opens the graph, **Then** it refuses with `PLACEMENT_NOT_GRANTED` before it fetches any segment.

Independent test: the bound test of `MEDULLA-OBJECT-STORAGE-BACKEND-R004`.

### User Story 5 - Archive batch segments by sequence range (Priority: P2)

The user is the archiver of point-in-time recovery. The outcome is immutable, digest-checked segments with no overlapping range.

**Acceptance scenarios**

1. **Given** a stored range, **When** the archiver puts an overlapping range, **Then** the engine refuses with `ARCHIVE_RANGE_OVERLAP`.
2. **Given** a stored segment is altered, **When** the archiver reads it, **Then** the engine refuses with `ARCHIVE_DIGEST_MISMATCH`.

Independent test: the bound test of `MEDULLA-OBJECT-STORAGE-BACKEND-R006`.

### User Story 6 - Catch unexplained cost growth across releases (Priority: P2)

The user is a release owner. The outcome is a gate that fails when object actions at open grow more than 10 percent.

**Acceptance scenarios**

1. **Given** a stamped run with an injected 15 percent growth of open actions, **When** the gate compares it with the previous release, **Then** the gate fails.
2. **Given** a run with no environment stamp, **When** the gate reads it, **Then** the run is claim-ineligible and never enters the evidence.

Independent test: the bound test of `MEDULLA-OBJECT-STORAGE-BACKEND-R005`.

### Edge Cases

- An `object_store` spec without a durability class refuses at open.
- A manifest naming a missing segment refuses with `OBJECT_SEGMENT_MISSING`; earlier versions stay readable.
- A local store ahead of `HEAD` is discarded and rebuilt; one behind `HEAD` replays the missing batches.
- A destination that already holds a graph refuses the copy with `GRAPH_COPY_DESTINATION_NOT_EMPTY`.
- A cost run with no pinned revision is claim-ineligible.

### Assumptions

- The object store offers create-only puts and conditional puts on a version tag.
- Raft remains the only commit authority; the object store holds the durable copy and the publish fence.
- Secrets are supplied by reference; the spec never holds an endpoint, bucket or root value.

## Requirements

- **FR-001**: The backend is declared by a typed spec and refuses before any effect. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R001`.
- **FR-002**: A graph opens from the root by verified, content-addressed segments. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R002`, `MEDULLA-OBJECT-STORAGE-BACKEND-R002.1`.
- **FR-003**: A commit acks only after the head update. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R003`, `MEDULLA-OBJECT-STORAGE-BACKEND-R003.1`.
- **FR-004**: A redb graph copies to the root with a verified digest. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R003.2`.
- **FR-005**: Several nodes open one root under the placement authority. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R004`.
- **FR-006**: Batch segments archive by sequence range. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R006`.
- **FR-007**: A comparative gate guards the fixed open cost. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R005`.

## Success Criteria

- **SC-001**: Every refusal of a backend spec leaves state unchanged in 100 percent of cases. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R001`.
- **SC-002**: A cold open restores every acknowledged write. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R002.1`.
- **SC-003**: No client is acked before the head update is durable. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R003.1`.
- **SC-004**: A graph copy runs within 400 ms p50 and 1,500 ms p99 per 100,000 elements. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R003.2`.
- **SC-005**: Growth of object actions at open above 10 percent fails the release gate. Rows: `MEDULLA-OBJECT-STORAGE-BACKEND-R005`.

## Requirements and acceptance

Every requirement ID this spec owns lives in [requirements.md](requirements.md): `MEDULLA-OBJECT-STORAGE-BACKEND-R001` to `R006`. Architecture and live wiring live in [plan.md](plan.md). The entities are in [data-model.md](data-model.md) and the shortest served proof is in [quickstart.md](quickstart.md). The spec accepts when every row reaches `VERIFIED`. A planned or tested item is not a landed item.

## Traceability

See [test-spec.md](test-spec.md).
