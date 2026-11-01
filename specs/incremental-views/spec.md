# MEDULLA-INCREMENTAL-VIEWS — Incremental view maintenance

Owner: medulla. Sole owner of incremental maintenance for plan views, SQL views and the ontology closure.
Cross-repo IDs: `ARBORA-OPS-R018` serves `CdcRead` and the SQL entry as generated operations; see [contracts/view-change-feed.md](contracts/view-change-feed.md).

## Delivery mapping

Component: Query
Depends-on: MEDULLA-ENGINE-PARITY-R014.1, MEDULLA-ENGINE-PARITY-R014.2, MEDULLA-ENGINE-PARITY-R015.1, MEDULLA-ENGINE-PARITY-R015.2
References: [MEDULLA-ENGINE-PARITY](../engine-parity/spec.md), [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md), [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/spec.md)
Code: ../../crates/medulla-exec/src/incremental/circuit.rs ../../crates/medulla-server-cdc/src/matview/manager.rs ../../crates/medulla-h-txn/src/dist_compute.rs ../../crates/medulla-plan/tests/incremental_execute_oracle.rs

## User Scenarios & Testing

### User Story 1 - Plan views stay fresh at the cost of the change (Priority: P1)

An agent defines a named view over one graph and reads it many times while the graph changes. The engine folds each change into the view, so a read never waits for a recompute. The agent trusts the answer because it equals a full recompute.

**Acceptance Scenarios**:

1. **Given** a view `Scan Doc` then `Filter year > 2000`, **When** a client adds, updates and removes nodes, **Then** each read equals a full recompute.
2. **Given** a view with a `Traverse` op, **When** the client defines it, **Then** the view runs in recompute mode with a typed reason.

### User Story 2 - SQL materialized views without refresh (Priority: P1)

A dashboard reads a grouped SQL materialized view over one table. Each commit updates the view in the same transaction. The dashboard never reads a stale flag and never issues a refresh.

**Acceptance Scenarios**:

1. **Given** `CREATE MATERIALIZED VIEW by_region AS SELECT region, count(*), sum(amount) FROM orders GROUP BY region`, **When** a client commits changes, **Then** the next read equals the defining query.
2. **Given** a defining query with `now()`, **When** the client creates the view, **Then** the view uses the refresh path and reports `MATVIEW_NOT_INCREMENTAL: now`.

### User Story 3 - Wrong answers are caught and contained (Priority: P1)

An operator runs many views in production. The engine samples views, recomputes them and compares. A mismatch demotes the view to recompute mode, counts a metric and logs the difference. Callers keep getting correct answers.

**Acceptance Scenarios**:

1. **Given** a view with corrupted state, **When** its check runs, **Then** the view demotes with `SHADOW_MISMATCH` and reads stay correct.

### User Story 4 - State stays inside a budget (Priority: P2)

An operator defines more views than fit in memory. The engine spills the least recently changed views to the store and reloads them on demand. A view whose own state exceeds its ceiling demotes with a typed reason.

**Acceptance Scenarios**:

1. **Given** a memory budget smaller than three views, **When** writes touch each graph in turn, **Then** one view spills and all reads stay correct.
2. **Given** a per-view ceiling of 1 MiB, **When** a view grows past it, **Then** it demotes with `STATE_BUDGET_EXCEEDED`.

### User Story 5 - Subscribers see what changed in an answer (Priority: P2)

A user interface subscribes to a view and receives result changes, not snapshots. A subscriber that falls behind reads a snapshot and resumes.

**Acceptance Scenarios**:

1. **Given** a subscriber at seq 0, **When** 100 changes commit, **Then** the folded feed equals the view result after each change.
2. **Given** a cursor below the retained ring, **When** the subscriber reads, **Then** it gets `gap: true`, reads the snapshot and resumes.

### User Story 6 - Ontology retraction costs the affected region (Priority: P3)

An ontology author removes one subclass axiom from a large ontology. The closure updates by removing only unsupported subsumptions and re-deriving the affected region.

**Acceptance Scenarios**:

1. **Given** a 10,000-class chain, **When** one middle edge is removed, **Then** the closure equals a fresh classification without a full rebuild.

### User Story 7 - Operators see how each view is maintained (Priority: P2)

An operator queries one system table and the metrics endpoint. Each view shows its mode, its reason, its state size and its check counts.

**Acceptance Scenarios**:

1. **Given** one supported and one unsupported view, **When** the operator selects from `medulla_catalog.matviews`, **Then** the rows show `incremental` and `recompute` with the code.

### Edge Cases

- An update that does not change a row's membership produces a consolidated delta of zero rows and no feed event.
- A transaction that inserts and deletes the same row consolidates to nothing.
- A weight sum that overflows `i64` refuses with `DELTA_WEIGHT_OVERFLOW`; a plan view demotes and a SQL commit refuses.
- A NULL filter result drops the row; a NULL aggregate argument changes no sum.
- A group whose count reaches zero leaves the view.
- A retraction of a group's current minimum recomputes only that group.
- A shadow check during a concurrent write skips and re-arms.
- A spilled view that changes reloads before it folds the change.
- A restart reseeds every plan view; no operator state from before the restart is trusted.
- Active row-level security refuses every plan view method, as today.

## Requirements

- **FR-001**: The engine maintains plan views from signed change deltas and serves the maintained result. Rows: `MEDULLA-INCREMENTAL-VIEWS-R001`, `MEDULLA-INCREMENTAL-VIEWS-R001.6`.
- **FR-002**: A plan the circuit cannot maintain runs in recompute mode with a typed reason. Rows: `MEDULLA-INCREMENTAL-VIEWS-R001.2`, `MEDULLA-INCREMENTAL-VIEWS-R001.7`.
- **FR-003**: One weighted delta type with consolidation serves every view family. Rows: `MEDULLA-INCREMENTAL-VIEWS-R002`.
- **FR-004**: One oracle harness proves every maintained construct, and a gate refuses a construct without a case. Rows: `MEDULLA-INCREMENTAL-VIEWS-R003`, `MEDULLA-INCREMENTAL-VIEWS-R004`.
- **FR-005**: Every view reports its mode, reason, state size and checks through one system table and metrics. Rows: `MEDULLA-INCREMENTAL-VIEWS-R005`.
- **FR-006**: Boot restores incremental mode by reseeding from the current graph. Rows: `MEDULLA-INCREMENTAL-VIEWS-R006`.
- **FR-007**: Operator state is counted, spilled under a process budget and demoted above a per-view ceiling. Rows: `MEDULLA-INCREMENTAL-VIEWS-R007`.
- **FR-008**: Sampled shadow verification demotes a view whose state disagrees with a recompute. Rows: `MEDULLA-INCREMENTAL-VIEWS-R008`.
- **FR-009**: SQL views over one table are maintained in the writing transaction. Rows: `MEDULLA-INCREMENTAL-VIEWS-R009`.
- **FR-010**: View result changes ride the existing change feed. Rows: `MEDULLA-INCREMENTAL-VIEWS-R010`.
- **FR-011**: The ontology closure updates from axiom inserts and retractions. Rows: `MEDULLA-INCREMENTAL-VIEWS-R011`.
- **FR-012**: SQL views support DISTINCT, min and max. Rows: `MEDULLA-INCREMENTAL-VIEWS-R012`.
- **FR-013**: SQL views support one indexed inner equi-join. Rows: `MEDULLA-INCREMENTAL-VIEWS-R013`.
- **FR-014**: A documentation page and a release gate cover the feature. Rows: `MEDULLA-INCREMENTAL-VIEWS-R014`, `MEDULLA-INCREMENTAL-VIEWS-R015`.

## Success Criteria

- **SC-001**: Maintenance touches at most 4 state entries per single-row change at any view size. Rows: `MEDULLA-INCREMENTAL-VIEWS-R001.5`.
- **SC-002**: A 1,000-row delta on a 1,000,000-row plan view applies in at most 20 ms p99. Rows: `MEDULLA-INCREMENTAL-VIEWS-R001.10`.
- **SC-003**: A SQL commit adds at most 50 ms p99 to reach a fresh view. Rows: `MEDULLA-INCREMENTAL-VIEWS-R009.7`, `MEDULLA-INCREMENTAL-VIEWS-R009.8`.
- **SC-004**: Every oracle shape agrees with a recompute after every step. Rows: `MEDULLA-INCREMENTAL-VIEWS-R009.6`, `MEDULLA-INCREMENTAL-VIEWS-R004`.
- **SC-005**: A corrupted view demotes within 2 s at a check rate of one in 10 changes. Rows: `MEDULLA-INCREMENTAL-VIEWS-R008.4`.
- **SC-006**: Resident view state never exceeds the memory budget plus one view. Rows: `MEDULLA-INCREMENTAL-VIEWS-R007.3`.

## Assumptions

- `MEDULLA-ENGINE-PARITY-R014.1` and `R014.2` deliver the SQL materialized view catalog, its result table and its refresh path before R009 starts.
- `MEDULLA-ENGINE-PARITY-R015.1` delivers the per-row old and new image list of a committed SQL transaction before R009.3 starts.
- Plan views stay refused under active row-level security, as the current admission rule states.
- UQL stays read-only. A plan view is defined through `PlanMatViewDefine` with the plan that UQL compiles to; UQL `EXPLAIN` reports maintainability.
- Incremental `sum` and `avg` serve exact numeric types only. Floats keep the refresh path.

## Why a new spec

The search ran `git grep -n -iE "incremental|maintainab|matview|materialized view|continuous quer|PlanMatView" -- specs`.
It read MEDULLA-FEDERATED-QUERY, MEDULLA-ENGINE-PARITY, MEDULLA-DURABLE-KERNEL, MEDULLA-UNIFIED-DATA-PLANE and MEDULLA-REPO-INGEST.

- MEDULLA-ENGINE-PARITY-R014 owns the SQL materialized view definition, its result table and the refresh path. It states no incremental maintenance.
- MEDULLA-ENGINE-PARITY-R015 owns table row-change capture. It states no view output.
- MEDULLA-FEDERATED-QUERY-R708 owns stored queries and their refresh policy. It states no delta kernel.
- MEDULLA-FEDERATED-QUERY FQR P1 names a maintainability reason in `EXPLAIN`. No row states it.
- MEDULLA-REPO-INGEST-R004 owns derived time series. It is a separate store.

No row owns the shipped delta kernel, the plan view manager, continuous queries or the incremental closure. The kernel serves both view families and the closure, so one spec keeps one owner for it. Each linked row stays with its owner and is cited by ID.

## Scope and interfaces

In scope: the delta kernel, plan view maintenance, SQL view maintenance, state budget and spill, shadow verification, status and metrics, the view change feed and incremental closure.

Interfaces: `Method::PlanMatViewDefine`, `PlanMatViewGet`, `PlanMatViewRefresh`, `PlanMatViewDrop`, `CdcRead`, `RegisterContinuousQuery`, UQL `EXPLAIN`, pgwire `CREATE MATERIALIZED VIEW` and the system table `medulla_catalog.matviews`.

Non-goals: the SQL view catalog and refresh (MEDULLA-ENGINE-PARITY-R014), row capture (MEDULLA-ENGINE-PARITY-R015), the durable feed protocol (MEDULLA-DURABLE-KERNEL-R043.100) and served operations (arbora).

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-INCREMENTAL-VIEWS-R001` | plan.md "Phase 1: Design" | T-001 |  |
| FR-002 | `MEDULLA-INCREMENTAL-VIEWS-R001.7` | plan.md "Phase 1: Design" | T-002 |  |
| FR-003 | `MEDULLA-INCREMENTAL-VIEWS-R002` | plan.md "Phase 1: Design" | T-003 |  |
| FR-004 | `MEDULLA-INCREMENTAL-VIEWS-R003` | plan.md "Phase 1: Design" | T-004 |  |
| FR-005 | `MEDULLA-INCREMENTAL-VIEWS-R005` | plan.md "Phase 1: Design" | T-005 |  |
| FR-006 | `MEDULLA-INCREMENTAL-VIEWS-R006` | plan.md "Phase 1: Design" | T-006 |  |
| FR-007 | `MEDULLA-INCREMENTAL-VIEWS-R007` | plan.md "Phase 1: Design" | T-007 |  |
| FR-008 | `MEDULLA-INCREMENTAL-VIEWS-R008` | plan.md "Phase 1: Design" | T-008 |  |
| FR-009 | `MEDULLA-INCREMENTAL-VIEWS-R009` | plan.md "Phase 1: Design" | T-009 |  |
| FR-010 | `MEDULLA-INCREMENTAL-VIEWS-R010` | plan.md "Cross-repo seams" | T-010 |  |
| FR-011 | `MEDULLA-INCREMENTAL-VIEWS-R011` | plan.md "Phase 1: Design" | T-011 |  |
| FR-012 | `MEDULLA-INCREMENTAL-VIEWS-R012` | plan.md "Phase 1: Design" | T-012 |  |
| FR-013 | `MEDULLA-INCREMENTAL-VIEWS-R013` | plan.md "Phase 1: Design" | T-013 |  |
| FR-014 | `MEDULLA-INCREMENTAL-VIEWS-R014` | plan.md "Build contract" | T-014 |  |
