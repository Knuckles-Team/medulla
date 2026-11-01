# MEDULLA-INCREMENTAL-VIEWS — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md). Rows: [requirements.md](requirements.md).

Each task line reads `- [ ] T001 [P] [US1] <ID>-R012.4 description`. The task number and the row ID are required.
`[P]` marks a task that may run in parallel. `[US1]` names the user story. Order is producer first. Compile slot: medulla only, one cargo lane host-wide at a time.

## First slice

The first slice is the unblocked frontier: rows whose dependencies are delivered today. It is R001.10, R002.1 to R002.4, R003, R004, R005.1 to R005.5, R006, R007.1 to R007.5 and R008.1 to R008.4. R009, R010 and R012 to R013 wait for MEDULLA-ENGINE-PARITY-R014.1, R014.2 and R015.1. R011.4 to R011.7 depend only on this spec and may start in parallel.

## Current state: bind the shipped kernel

- [ ] T001 [US1] MEDULLA-INCREMENTAL-VIEWS-R001 Plan views are maintained from change deltas (rollup). Check the root test.
- [x] T002 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.1 Signed row deltas. Bind the shipped test.
- [x] T003 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.2 Compile or name the first unsupported op. Bind the shipped test.
- [x] T004 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.3 Membership views equal the executor after every change. Bind the shipped test.
- [x] T005 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.4 Tumbling window aggregates equal the executor. Bind the shipped test.
- [x] T006 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.5 Maintenance cost follows the delta, not the view. Bind the shipped test.
- [x] T007 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.6 Define, change and read are wired. Bind the shipped test.
- [x] T008 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.7 A fallback keeps a typed reason. Bind the shipped test.
- [x] T009 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.8 EXPLAIN reports maintainability. Bind the shipped test.
- [x] T010 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.9 Continuous aggregate queries fold the change feed. Bind the shipped test.
- [ ] T011 [US1] MEDULLA-INCREMENTAL-VIEWS-R001.10 Served hot-path proof for plan views. Implement it with its tests.

## Delta type and oracle

- [ ] T012 [US1] MEDULLA-INCREMENTAL-VIEWS-R002 One weighted delta type for every view family (rollup). Check the root test.
- [ ] T013 [US1] MEDULLA-INCREMENTAL-VIEWS-R002.1 Move and widen the delta type. Implement it with its tests.
- [ ] T014 [US1] MEDULLA-INCREMENTAL-VIEWS-R002.2 Consolidate and refuse overflow. Implement it with its tests.
- [ ] T015 [US1] MEDULLA-INCREMENTAL-VIEWS-R002.3 Wiring: the circuit consolidates before it folds. Implement it with its tests.
- [ ] T016 [US1] MEDULLA-INCREMENTAL-VIEWS-R002.4 Proof: shipped graph behavior is unchanged. Implement it with its tests.
- [ ] T017 [P] [US1] MEDULLA-INCREMENTAL-VIEWS-R003 One differential oracle harness. Implement it with its tests.
- [ ] T018 [US1] MEDULLA-INCREMENTAL-VIEWS-R004 No operator ships without an oracle case. Implement it with its tests.

## Status, restart, budget and verification

- [ ] T019 [US7] MEDULLA-INCREMENTAL-VIEWS-R005 Maintenance state is observable (rollup). Check the root test.
- [ ] T020 [P] [US7] MEDULLA-INCREMENTAL-VIEWS-R005.1 Model: the view status board. Implement it with its tests.
- [ ] T021 [US7] MEDULLA-INCREMENTAL-VIEWS-R005.2 The manager publishes every state change. Implement it with its tests.
- [ ] T022 [US7] MEDULLA-INCREMENTAL-VIEWS-R005.3 System table `medulla_catalog.matviews`. Implement it with its tests.
- [ ] T023 [P] [US7] MEDULLA-INCREMENTAL-VIEWS-R005.4 Prometheus view metrics. Implement it with its tests.
- [ ] T024 [US7] MEDULLA-INCREMENTAL-VIEWS-R005.5 Served proof of the status table. Implement it with its tests.
- [ ] T025 [US1] MEDULLA-INCREMENTAL-VIEWS-R006 Boot restores incremental mode by reseeding. Implement it with its tests.
- [ ] T026 [US4] MEDULLA-INCREMENTAL-VIEWS-R007 Operator state is budgeted and spilled (rollup). Check the root test.
- [ ] T027 [P] [US4] MEDULLA-INCREMENTAL-VIEWS-R007.1 State accounting. Implement it with its tests.
- [ ] T028 [US4] MEDULLA-INCREMENTAL-VIEWS-R007.2 Per-view ceiling demotes. Implement it with its tests.
- [ ] T029 [US4] MEDULLA-INCREMENTAL-VIEWS-R007.3 Spill and reload by least recent change. Implement it with its tests.
- [ ] T030 [US4] MEDULLA-INCREMENTAL-VIEWS-R007.4 Wiring: the store backs the spill. Implement it with its tests.
- [ ] T031 [US4] MEDULLA-INCREMENTAL-VIEWS-R007.5 Served proof of budget and spill. Implement it with its tests.
- [ ] T032 [US3] MEDULLA-INCREMENTAL-VIEWS-R008 Sampled shadow verification in production (rollup). Check the root test.
- [ ] T033 [P] [US3] MEDULLA-INCREMENTAL-VIEWS-R008.1 Model: the check schedule. Implement it with its tests.
- [ ] T034 [US3] MEDULLA-INCREMENTAL-VIEWS-R008.2 Compare and demote. Implement it with its tests.
- [ ] T035 [US3] MEDULLA-INCREMENTAL-VIEWS-R008.3 Wiring: the check task. Implement it with its tests.
- [ ] T036 [US3] MEDULLA-INCREMENTAL-VIEWS-R008.4 Served proof of shadow verification. Implement it with its tests.

## SQL views

- [ ] T037 [US2] MEDULLA-INCREMENTAL-VIEWS-R009 SQL views over one table are maintained in the commit (rollup). Check the root test.
- [ ] T038 [P] [US2] MEDULLA-INCREMENTAL-VIEWS-R009.1 Model: the incremental view program. Implement it with its tests.
- [ ] T039 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.2 Refusal: name the first unsupported construct. Implement it with its tests.
- [ ] T040 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.3 Behavior: linear maintenance in the commit. Implement it with its tests.
- [ ] T041 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.4 Behavior: grouped exact aggregates. Implement it with its tests.
- [ ] T042 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.5 Wiring: create, skip the stale flag, publish status. Implement it with its tests.
- [ ] T043 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.6 SQL differential oracle. Implement it with its tests.
- [ ] T044 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.7 Served hot-path proof for SQL views. Implement it with its tests.
- [ ] T045 [US2] MEDULLA-INCREMENTAL-VIEWS-R009.8 Nightly budget for SQL views. Implement it with its tests.

## View change feed

- [ ] T046 [US5] MEDULLA-INCREMENTAL-VIEWS-R010 View changes ride the one change feed (rollup). Check the root test.
- [ ] T047 [US5] MEDULLA-INCREMENTAL-VIEWS-R010.1 Plan views publish their output delta. Implement it with its tests.
- [ ] T048 [US5] MEDULLA-INCREMENTAL-VIEWS-R010.2 SQL views publish their output delta. Implement it with its tests.
- [ ] T049 [US5] MEDULLA-INCREMENTAL-VIEWS-R010.3 Resume, expiry and snapshot. Implement it with its tests.
- [ ] T050 [US5] MEDULLA-INCREMENTAL-VIEWS-R010.4 Served proof and arbora seam. Implement it with its tests.

## Ontology closure

- [ ] T051 [US6] MEDULLA-INCREMENTAL-VIEWS-R011 Ontology closure updates from fact deltas (rollup). Check the root test.
- [x] T052 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.1 Monotone axiom insert is incremental. Bind the shipped test.
- [x] T053 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.2 Writes on opted-in graphs refresh the closure. Bind the shipped test.
- [x] T054 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.3 A retraction never serves a stale entailment. Bind the shipped test.
- [ ] T055 [P] [US6] MEDULLA-INCREMENTAL-VIEWS-R011.4 Model: a reverse index from causes to derived subsumptions. Implement it with its tests.
- [ ] T056 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.5 Behavior: delete and re-derive. Implement it with its tests.
- [ ] T057 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.6 Wiring: the cascade retracts instead of rebuilding. Implement it with its tests.
- [ ] T058 [US6] MEDULLA-INCREMENTAL-VIEWS-R011.7 Hot-path proof for retraction. Implement it with its tests.

## More SQL shapes

- [ ] T059 [US2] MEDULLA-INCREMENTAL-VIEWS-R012 SQL views gain DISTINCT, min and max (rollup). Check the root test.
- [ ] T060 [P] [US2] MEDULLA-INCREMENTAL-VIEWS-R012.1 Model: extreme aggregates and distinct. Implement it with its tests.
- [ ] T061 [US2] MEDULLA-INCREMENTAL-VIEWS-R012.2 Behavior: insert updates, extreme retraction recomputes the group. Implement it with its tests.
- [ ] T062 [US2] MEDULLA-INCREMENTAL-VIEWS-R012.3 Wiring: the compiler accepts them. Implement it with its tests.
- [ ] T063 [US2] MEDULLA-INCREMENTAL-VIEWS-R012.4 Oracle cases. Implement it with its tests.
- [ ] T064 [US2] MEDULLA-INCREMENTAL-VIEWS-R012.5 Served proof. Implement it with its tests.
- [ ] T065 [US2] MEDULLA-INCREMENTAL-VIEWS-R013 SQL views over a two-table inner equi-join (rollup). Check the root test.
- [ ] T066 [P] [US2] MEDULLA-INCREMENTAL-VIEWS-R013.1 Model: join program. Implement it with its tests.
- [ ] T067 [US2] MEDULLA-INCREMENTAL-VIEWS-R013.2 Behavior: the join delta from post-commit tables. Implement it with its tests.
- [ ] T068 [US2] MEDULLA-INCREMENTAL-VIEWS-R013.3 Wiring and refusal: the compiler accepts one indexed equi-join. Implement it with its tests.
- [ ] T069 [US2] MEDULLA-INCREMENTAL-VIEWS-R013.4 Oracle case. Implement it with its tests.
- [ ] T070 [US2] MEDULLA-INCREMENTAL-VIEWS-R013.5 Served proof. Implement it with its tests.

## Documentation, release and evidence

- [ ] T071 [P] [US7] MEDULLA-INCREMENTAL-VIEWS-R014 Documentation and quickstart. Implement it with its tests.
- [ ] T072 [US7] MEDULLA-INCREMENTAL-VIEWS-R015 Release gate. Implement it with its tests.
- [ ] T073 [US7] MEDULLA-INCREMENTAL-VIEWS-R015 Evidence: reconcile every row against merged code, bound tests and the documentation page.
