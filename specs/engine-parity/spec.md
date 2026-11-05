# MEDULLA-ENGINE-PARITY — Relational engine parity inside one library

Component: Storage

## Outcome

The engine stays one library: the embedded transactional store plus the columnar query engine. It gains the strengths of a mature relational database inside that library. No second database engine joins it, and no data syncs into one.

- Writers scale through group commit and optimistic concurrency.
- Outside SQL and BI tools connect over the PostgreSQL wire protocol under engine principals and row security.
- User tables gain secondary, partial and expression indexes, constraints, triggers, sequences and materialized views.
- The committed log serves change capture and point-in-time recovery.
- Outside PostgreSQL databases stay attached sources. The engine reads and federates them only.

Each capability is built only when a named consumer needs it.

## Scope

Owner: `medulla` (native Rust), sole owner. The code lands in `crates/medulla-persistence` (group commit), `crates/medulla-transaction` (footprints), `crates/medulla-table-store` and `crates/medulla-tables` (user-table features), `crates/medulla-sql`, `crates/medulla-sql-lang` and `crates/medulla-h-query` (catalog, classifier, dispatch), `crates/medulla-wire` and `crates/medulla-wire-core` (pgwire), `crates/medulla-server-cdc` (row capture) and `crates/medulla-storage` (archive). Non-goals: no second database engine, no columnar copy, no read replica built on a second change stream (read scale comes from Raft followers and the shared object root of `MEDULLA-OBJECT-STORAGE-BACKEND`), and no engine data in an outside database. Arbora issues outside client identities (`ARBORA-OPS-R858`).

Why a spec: no other spec owns write scaling across store files, the wire message flows, catalog emulation, the dialect matrix or the relational features of user tables. Neighbours are linked by row ID: `MEDULLA-DURABLE-KERNEL-R035` owns the wire operational surfaces, `MEDULLA-DURABLE-KERNEL-R084` owns version-checked writes, and `MEDULLA-UNIFIED-DATA-PLANE-R013` owns the outside PostgreSQL adapter as an attached source.

## User Scenarios & Testing

### User Story 1 - Many writers commit without waiting on each other (Priority: P1)

The user is an application that writes through the engine from many sessions. The outcome is high commit throughput with typed conflicts, so that disjoint writers never block.

**Acceptance scenarios**

1. **Given** 64 concurrent single-row writers, **When** the writer drains its queue, **Then** one commit and one fsync serve the batch and each caller receives its own receipt.
2. **Given** two transactions touch the same row, **When** the second commits, **Then** it refuses with `TXN_CONFLICT` and writes nothing.
3. **Given** the commit queue stays full past its timeout, **When** a writer enqueues, **Then** it refuses with `COMMIT_QUEUE_FULL` and changes no state.

Independent test: the bound tests of `MEDULLA-ENGINE-PARITY-R001.2`, `MEDULLA-ENGINE-PARITY-R002.2` and `MEDULLA-ENGINE-PARITY-R004.2`.

### User Story 2 - Connect a standard SQL tool under engine principals (Priority: P1)

The user is an analyst with a PostgreSQL client or BI tool. The outcome is a connection that introspects, prepares and copies under the same row security as a native call.

**Acceptance scenarios**

1. **Given** a wire login maps to an engine principal, **When** the client reads a tagged table, **Then** it sees the same rows as the native `Uql` call for that principal.
2. **Given** a client sends an unsupported message or a refused statement form, **When** the engine classifies it, **Then** it answers SQLSTATE `0A000` naming the matrix entry.
3. **Given** a BI tool replays its connect queries, **When** the catalog answers, **Then** it lists only tables the principal may read.

Independent test: the bound tests of `MEDULLA-ENGINE-PARITY-R006.2`, `MEDULLA-ENGINE-PARITY-R007.2`, `MEDULLA-ENGINE-PARITY-R008.2` and `MEDULLA-ENGINE-PARITY-R009.2`.

### User Story 3 - Use relational features on user tables (Priority: P2)

The user is a developer who models data in user tables. The outcome is indexes, constraints, triggers, sequences and materialized views with typed refusals.

**Acceptance scenarios**

1. **Given** a partial index, **When** a query's predicate matches it, **Then** the planner uses the index and `EXPLAIN` names it.
2. **Given** a violated constraint, **When** the transaction commits, **Then** the engine returns the SQLSTATE and the constraint name and leaves the table unchanged.
3. **Given** a before-insert trigger and a sequence, **When** rows insert, **Then** the trigger rewrites the row once and values never repeat after a crash.
4. **Given** a materialized view over a table, **When** an input changes, **Then** the view reports stale until refreshed.

Independent test: the bound tests of `MEDULLA-ENGINE-PARITY-R010.2`, `MEDULLA-ENGINE-PARITY-R011.2`, `MEDULLA-ENGINE-PARITY-R012.2`, `MEDULLA-ENGINE-PARITY-R013.2` and `MEDULLA-ENGINE-PARITY-R014.2`.

### User Story 4 - Capture changes and recover to a point in time (Priority: P2)

The user is a subscriber or an operator. The outcome is an ordered change feed that includes user-table rows and a restore to a chosen sequence or time.

**Acceptance scenarios**

1. **Given** SQL writes commit, **When** a subscriber reads from a cursor, **Then** it receives the row changes in commit order and resumes from its cursor.
2. **Given** archived batch segments after a backup bundle, **When** the operator restores to a time, **Then** the store matches the state at that sequence.
3. **Given** a target after the last archived record, **When** the operator restores, **Then** the engine refuses with `RECOVERY_TARGET_UNAVAILABLE` and writes nothing.

Independent test: the bound tests of `MEDULLA-ENGINE-PARITY-R015.2` and `MEDULLA-ENGINE-PARITY-R016.2`.

### User Story 5 - Keep outside databases as sources and report capabilities (Priority: P2)

The user is an operator. The outcome is a closed storage set, a refusal to place engine data outside, and a capability report that matches the build.

**Acceptance scenarios**

1. **Given** an attached outside database, **When** a write-back targets an engine record class, **Then** the engine refuses with `EXTERNAL_PLACEMENT_FORBIDDEN` and writes nothing to the source.
2. **Given** a running engine, **When** a client asks for `EngineCapabilities`, **Then** the report matches the compiled features and the pgwire `server_version`.

Independent test: the bound tests of `MEDULLA-ENGINE-PARITY-R019.2` and `MEDULLA-ENGINE-PARITY-R020.2`.

### Edge Cases

- An explicit client transaction never retries a conflict; the client receives SQLSTATE `40001`.
- A footprint older than the history ring refuses with `TXN_SNAPSHOT_EXPIRED`.
- A wire login with a revoked record refuses with SQLSTATE `28000` within the cache TTL.
- A trigger chain deeper than 16 refuses with `TRIGGER_DEPTH`.
- A change cursor below the retained window refuses with `CURSOR_EXPIRED`.
- A missing or corrupt archive segment refuses the restore and names the range.

### Assumptions

- Raft followers and the shared object root provide read scale; this spec adds no replica mode and no columnar copy.
- Benchmarks run in the nightly suite on the soak host and never gate a pull request.
- Arbora issues outside client identities; the engine stores only verifiers, never secrets.

## Requirements

- **FR-001**: Group commit with a bounded enqueue wait. Rows: `MEDULLA-ENGINE-PARITY-R001`, `MEDULLA-ENGINE-PARITY-R001.1`, `MEDULLA-ENGINE-PARITY-R001.2`, `MEDULLA-ENGINE-PARITY-R001.3`.
- **FR-002**: Optimistic concurrency and lock-free acceptance for disjoint footprints. Rows: `MEDULLA-ENGINE-PARITY-R002`, `MEDULLA-ENGINE-PARITY-R004`.
- **FR-003**: The PostgreSQL wire front end with one SQLSTATE table and wire identities. Rows: `MEDULLA-ENGINE-PARITY-R006`, `MEDULLA-ENGINE-PARITY-R007`.
- **FR-004**: Catalog views and a dialect compatibility matrix. Rows: `MEDULLA-ENGINE-PARITY-R008`, `MEDULLA-ENGINE-PARITY-R009`.
- **FR-005**: Indexes, constraints, triggers, sequences and materialized views on user tables. Rows: `MEDULLA-ENGINE-PARITY-R010`, `MEDULLA-ENGINE-PARITY-R011`, `MEDULLA-ENGINE-PARITY-R012`, `MEDULLA-ENGINE-PARITY-R013`, `MEDULLA-ENGINE-PARITY-R014`.
- **FR-006**: Change capture and point-in-time recovery over the committed log. Rows: `MEDULLA-ENGINE-PARITY-R015`, `MEDULLA-ENGINE-PARITY-R016`.
- **FR-007**: Outside databases are sources only. Rows: `MEDULLA-ENGINE-PARITY-R019`.
- **FR-008**: A capability report and a write benchmark suite. Rows: `MEDULLA-ENGINE-PARITY-R005`, `MEDULLA-ENGINE-PARITY-R020`.

## Success Criteria

- **SC-001**: 64 concurrent writers reach at least 20,000 commits per second on one store file. Rows: `MEDULLA-ENGINE-PARITY-R001.3`.
- **SC-002**: Disjoint transactions return no `40001` under 32 sessions. Rows: `MEDULLA-ENGINE-PARITY-R004.3`.
- **SC-003**: Wire reads equal native reads under row security for every principal in the fixture. Rows: `MEDULLA-ENGINE-PARITY-R007.2`.
- **SC-004**: Every supported entry of the compatibility matrix passes its recorded statement. Rows: `MEDULLA-ENGINE-PARITY-R009.3`.
- **SC-005**: A restore loses no committed sequence up to its target. Rows: `MEDULLA-ENGINE-PARITY-R016.3`.
- **SC-006**: No engine record reaches an outside database. Rows: `MEDULLA-ENGINE-PARITY-R019.2`.

## Requirements and acceptance

Every requirement ID lives in [requirements.md](requirements.md): `MEDULLA-ENGINE-PARITY-R001` to `R020`. Rows `R003`, `R017` and `R018` are retired. Architecture and live wiring live in [plan.md](plan.md). The entities are in [data-model.md](data-model.md) and the shortest served proof is in [quickstart.md](quickstart.md). The spec accepts when every row reaches `VERIFIED`. A planned or tested item is not a landed item.

## Traceability

See [test-spec.md](test-spec.md).
