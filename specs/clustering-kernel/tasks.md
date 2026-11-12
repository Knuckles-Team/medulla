# MEDULLA-CLUSTER — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md). Rows: [requirements.md](requirements.md).

Each task line reads `- [ ] T001 [P] [US1] <ID>-R012.4 description`. The task number and the row ID are required.
`[P]` marks a task that may run in parallel. `[US1]` names the user story. Order is producer first. Compile slot: medulla only, one cargo lane host-wide at a time.

## Delivery slices

- Slice A (PR 2): kernel, state machine, refusals, knobs, memory bound. No caller changes.
- Slice B (PR 3): caller migration and deletions. The baseline of the old method is recorded in research.md D7 first.
- Slice C (PR 4): online refresh, build off the locks, benchmark acceptance.

The spec itself lands first as PR 1.

## Slice A: kernel

- [ ] T001 [US1] MEDULLA-CLUSTER-R001 One kernel owns centroid training (rollup).
- [ ] T002 [US1] MEDULLA-CLUSTER-R001.1 Module and types.
- [ ] T003 [US1] MEDULLA-CLUSTER-R001.2 Typed refusals.
- [ ] T004 [US1] MEDULLA-CLUSTER-R002 E-means with scaling-law defaults (rollup).
- [ ] T005 [US1] MEDULLA-CLUSTER-R002.1 Scaling law.
- [ ] T006 [US1] MEDULLA-CLUSTER-R002.2 Seeding.
- [ ] T007 [US1] MEDULLA-CLUSTER-R002.3 Mini-batch moving-average update.
- [ ] T008 [US1] MEDULLA-CLUSTER-R002.4 Exact mode.
- [ ] T009 [P] [US1] MEDULLA-CLUSTER-R002.5 Every tunable is a knob.
- [ ] T010 [US1] MEDULLA-CLUSTER-R003 Bit-identical results (rollup).
- [ ] T011 [US1] MEDULLA-CLUSTER-R003.1 Fixed reduction order.
- [ ] T012 [US1] MEDULLA-CLUSTER-R003.2 Thread-count independence.
- [ ] T013 [US2] MEDULLA-CLUSTER-R004 Explicit run state machine (rollup).
- [ ] T014 [US2] MEDULLA-CLUSTER-R004.1 Transition table.
- [ ] T015 [US2] MEDULLA-CLUSTER-R004.2 Every transition is tested.
- [ ] T016 [US2] MEDULLA-CLUSTER-R004.4 Cancellation.
- [ ] T017 [US1] MEDULLA-CLUSTER-R005 Bounded work (rollup).
- [ ] T018 [US1] MEDULLA-CLUSTER-R005.1 Input refusals.
- [ ] T019 [US1] MEDULLA-CLUSTER-R005.2 Step budget.
- [ ] T020 [US1] MEDULLA-CLUSTER-R005.3 Deterministic empty-cluster reseed.
- [ ] T021 [US1] MEDULLA-CLUSTER-R005.4 Too few distinct rows.
- [ ] T022 [US1] MEDULLA-CLUSTER-R006 Bounded training memory (rollup).
- [ ] T023 [US1] MEDULLA-CLUSTER-R006.1 Formula.
- [ ] T024 [US1] MEDULLA-CLUSTER-R006.2 Refuse before allocation.
- [ ] T025 [P] [US1] MEDULLA-CLUSTER-R006.3 Measured peak.
- [ ] T026 [US1] MEDULLA-CLUSTER-R007 Baseline instruction set, native Rust (rollup).
- [ ] T027 [P] [US1] MEDULLA-CLUSTER-R007.1 No compile-time CPU pin, no FFI.
- [ ] T028 [US2] MEDULLA-CLUSTER-R011.3 Bounded compute pool.

## Slice B: callers and deletions

- [ ] T029 [US1] MEDULLA-CLUSTER-R001.3 Wiring: the served data-science method runs the kernel.
- [ ] T030 [US3] MEDULLA-CLUSTER-R008.2 Distance helpers move to the kernel.
- [ ] T031 [US3] MEDULLA-CLUSTER-R008.1 IVF coarse quantizer and PQ codebooks train on the kernel.
- [ ] T032 [US1] MEDULLA-CLUSTER-R003.3 Index codebooks reproduce.
- [ ] T033 [US3] MEDULLA-CLUSTER-R008.3 The accelerator becomes a kernel backend.
- [ ] T034 [US3] MEDULLA-CLUSTER-R008.4 The served data-science method.
- [ ] T035 [US3] MEDULLA-CLUSTER-R008.5 The SQL aggregate and the Python kernel.
- [ ] T036 [US3] MEDULLA-CLUSTER-R008.6 Spectral partitions (MEDULLA-FEDERATED-QUERY-R607.2).
- [ ] T037 [US3] MEDULLA-CLUSTER-R008.7 Mixture-model seeding.
- [ ] T038 [US3] MEDULLA-CLUSTER-R008 Every caller runs on the kernel (rollup).
- [ ] T039 [US3] MEDULLA-CLUSTER-R008.8 Sweep for leftovers.
- [ ] T040 [US1] MEDULLA-CLUSTER-R001.4 Proof: only the kernel trains.
- [ ] T041 [US1] MEDULLA-CLUSTER-R002.6 Wiring: the engine resolves knobs once per build.
- [ ] T042 [US1] MEDULLA-CLUSTER-R007.2 Runtime-detected wide path.

## Slice C: online refresh, safe build, benchmark

- [ ] T043 [US1] MEDULLA-CLUSTER-R010.1 Bench harness.
- [ ] T044 [US4] MEDULLA-CLUSTER-R009 Online centroid refresh with drift-triggered retrain (rollup).
- [ ] T045 [US4] MEDULLA-CLUSTER-R009.1 Centroid tracker.
- [ ] T046 [US4] MEDULLA-CLUSTER-R009.2 Drift ratio.
- [ ] T047 [US2] MEDULLA-CLUSTER-R011 A build never stalls the engine (rollup).
- [ ] T048 [US2] MEDULLA-CLUSTER-R011.1 Build off the store locks.
- [ ] T049 [US2] MEDULLA-CLUSTER-R011.2 Writer latency bound.
- [ ] T050 [US2] MEDULLA-CLUSTER-R011.4 Shutdown cancels the build.
- [ ] T051 [US2] MEDULLA-CLUSTER-R011.5 Readiness does not wait for the build.
- [ ] T052 [US4] MEDULLA-CLUSTER-R009.3 Wiring: ingest feeds the tracker.
- [ ] T053 [US4] MEDULLA-CLUSTER-R009.4 Background retrain, warm start, swap.
- [ ] T054 [US1] MEDULLA-CLUSTER-R010 Benchmark against the current full-batch method (rollup).
- [ ] T055 [US1] MEDULLA-CLUSTER-R010.2 Acceptance thresholds.

## Bindings

- [ ] T056 [US5] MEDULLA-CLUSTER-R004.3 T-OBSERVE-BIND: when `specs/engine-observability/` lands, name its background-task registry provider row by ID in R004.3 and in contracts/progress-sink.md, then replace the log sink in `activate_one` with the registry adapter. Never edit the other spec.
