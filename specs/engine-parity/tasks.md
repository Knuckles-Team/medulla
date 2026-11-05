# MEDULLA-ENGINE-PARITY — Tasks

One task per leaf row of `requirements.md`, producer first within each user story. A rollup row has no task of its own; its children carry the work. Each task names its row; the row holds the files, the caller chain and the proof. Rows that edit `crates/medulla-table-store/src/tables/store.rs` run in series. Compile slot: Medulla only, one cargo lane host-wide at a time.

## Tasks

- [ ] T001 [US1] MEDULLA-ENGINE-PARITY-R001.1 Group commit with a bounded enqueue wait: model and behavior
- [ ] T002 [US1] MEDULLA-ENGINE-PARITY-R001.2 Group commit with a bounded enqueue wait: wiring and refusal
- [ ] T003 [P] [US1] MEDULLA-ENGINE-PARITY-R001.3 Group commit with a bounded enqueue wait: hot-path proof
- [ ] T004 [US1] MEDULLA-ENGINE-PARITY-R002.1 Optimistic concurrency for user-table transactions: model and behavior
- [ ] T005 [US1] MEDULLA-ENGINE-PARITY-R002.2 Optimistic concurrency for user-table transactions: wiring and refusal
- [ ] T006 [P] [US1] MEDULLA-ENGINE-PARITY-R002.3 Optimistic concurrency for user-table transactions: hot-path proof
- [ ] T007 [US1] MEDULLA-ENGINE-PARITY-R004.1 Disjoint transactions commit without a global lock: model and behavior
- [ ] T008 [US1] MEDULLA-ENGINE-PARITY-R004.2 Disjoint transactions commit without a global lock: wiring and refusal
- [ ] T009 [P] [US1] MEDULLA-ENGINE-PARITY-R004.3 Disjoint transactions commit without a global lock: hot-path proof
- [ ] T010 [US1] MEDULLA-ENGINE-PARITY-R005.1 Write throughput benchmark suite and targets: model and behavior
- [ ] T011 [US1] MEDULLA-ENGINE-PARITY-R005.2 Write throughput benchmark suite and targets: wiring and refusal
- [ ] T012 [P] [US1] MEDULLA-ENGINE-PARITY-R005.3 Write throughput benchmark suite and targets: hot-path proof
- [ ] T013 [US2] MEDULLA-ENGINE-PARITY-R006.1 PostgreSQL wire protocol front end: model and behavior
- [ ] T014 [US2] MEDULLA-ENGINE-PARITY-R006.2 PostgreSQL wire protocol front end: wiring and refusal
- [ ] T015 [P] [US2] MEDULLA-ENGINE-PARITY-R006.3 PostgreSQL wire protocol front end: hot-path proof
- [ ] T016 [US2] MEDULLA-ENGINE-PARITY-R007.1 Wire authentication mapped to engine principals and row security: model and behavior
- [ ] T017 [US2] MEDULLA-ENGINE-PARITY-R007.2 Wire authentication mapped to engine principals and row security: wiring and refusal
- [ ] T018 [P] [US2] MEDULLA-ENGINE-PARITY-R007.3 Wire authentication mapped to engine principals and row security: hot-path proof
- [ ] T019 [US2] MEDULLA-ENGINE-PARITY-R007.4 Wire authentication mapped to engine principals and row security: wiring: grant and revoke wire logins
- [ ] T020 [US2] MEDULLA-ENGINE-PARITY-R008.1 Catalog views for outside tools: model and behavior
- [ ] T021 [US2] MEDULLA-ENGINE-PARITY-R008.2 Catalog views for outside tools: wiring and refusal
- [ ] T022 [P] [US2] MEDULLA-ENGINE-PARITY-R008.3 Catalog views for outside tools: hot-path proof
- [ ] T023 [US2] MEDULLA-ENGINE-PARITY-R009.1 SQL dialect compatibility matrix: model and behavior
- [ ] T024 [US2] MEDULLA-ENGINE-PARITY-R009.2 SQL dialect compatibility matrix: wiring and refusal
- [ ] T025 [P] [US2] MEDULLA-ENGINE-PARITY-R009.3 SQL dialect compatibility matrix: hot-path proof
- [ ] T026 [US3] MEDULLA-ENGINE-PARITY-R010.1 Partial and expression indexes on user tables: model and behavior
- [ ] T027 [US3] MEDULLA-ENGINE-PARITY-R010.2 Partial and expression indexes on user tables: wiring and refusal
- [ ] T028 [P] [US3] MEDULLA-ENGINE-PARITY-R010.3 Partial and expression indexes on user tables: hot-path proof
- [ ] T029 [US3] MEDULLA-ENGINE-PARITY-R011.1 Constraints: unique, foreign key, check and not null: model and behavior
- [ ] T030 [US3] MEDULLA-ENGINE-PARITY-R011.2 Constraints: unique, foreign key, check and not null: wiring and refusal
- [ ] T031 [P] [US3] MEDULLA-ENGINE-PARITY-R011.3 Constraints: unique, foreign key, check and not null: hot-path proof
- [ ] T032 [US3] MEDULLA-ENGINE-PARITY-R012.1 Triggers as statechart rules: model and behavior
- [ ] T033 [US3] MEDULLA-ENGINE-PARITY-R012.2 Triggers as statechart rules: wiring and refusal
- [ ] T034 [P] [US3] MEDULLA-ENGINE-PARITY-R012.3 Triggers as statechart rules: hot-path proof
- [ ] T035 [US3] MEDULLA-ENGINE-PARITY-R013.1 Named sequences: model and behavior
- [ ] T036 [US3] MEDULLA-ENGINE-PARITY-R013.2 Named sequences: wiring and refusal
- [ ] T037 [P] [US3] MEDULLA-ENGINE-PARITY-R013.3 Named sequences: hot-path proof
- [ ] T038 [US3] MEDULLA-ENGINE-PARITY-R014.1 Views and materialized views over tables: model and behavior
- [ ] T039 [US3] MEDULLA-ENGINE-PARITY-R014.2 Views and materialized views over tables: wiring and refusal
- [ ] T040 [P] [US3] MEDULLA-ENGINE-PARITY-R014.3 Views and materialized views over tables: hot-path proof
- [ ] T041 [US4] MEDULLA-ENGINE-PARITY-R015.1 Change data capture for graph and table rows: model and behavior
- [ ] T042 [US4] MEDULLA-ENGINE-PARITY-R015.2 Change data capture for graph and table rows: wiring and refusal
- [ ] T043 [P] [US4] MEDULLA-ENGINE-PARITY-R015.3 Change data capture for graph and table rows: hot-path proof
- [ ] T044 [US4] MEDULLA-ENGINE-PARITY-R016.1 Point-in-time recovery from a backup bundle and archived batches: model and behavior
- [ ] T045 [US4] MEDULLA-ENGINE-PARITY-R016.2 Point-in-time recovery from a backup bundle and archived batches: wiring and refusal
- [ ] T046 [P] [US4] MEDULLA-ENGINE-PARITY-R016.3 Point-in-time recovery from a backup bundle and archived batches: hot-path proof
- [ ] T047 [US5] MEDULLA-ENGINE-PARITY-R019.1 Outside PostgreSQL databases are sources only: model and behavior
- [ ] T048 [US5] MEDULLA-ENGINE-PARITY-R019.2 Outside PostgreSQL databases are sources only: wiring and refusal
- [ ] T049 [P] [US5] MEDULLA-ENGINE-PARITY-R019.3 Outside PostgreSQL databases are sources only: hot-path proof
- [ ] T050 [US5] MEDULLA-ENGINE-PARITY-R020.1 Capability report: model and behavior
- [ ] T051 [US5] MEDULLA-ENGINE-PARITY-R020.2 Capability report: wiring and refusal
