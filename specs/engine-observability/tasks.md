# MEDULLA-OBSERVE — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md). Rows: [requirements.md](requirements.md).

Each task line reads `- [ ] T001 [P] [US1] <ID>-R012.4 description`. The task number and the row ID are required.
`[P]` marks a task that may run in parallel. `[US1]` names the user story. Order is producer first. Compile slot: medulla only, one cargo lane host-wide at a time.

## First slice

The first slice is slice A, then slice B. Slice C is two rows in MEDULLA-DURABLE-KERNEL and lands between B and D. Slice D adds symbols. Slice E adds the watchdog and the profiler. Slice F is the chaos proof. Rows R003.3 to R003.6 attach the registry to existing jobs and may run in parallel once R003.2 lands.


## Slice A: typed refusals, module targets, trace id

- [ ] T001 [US6] MEDULLA-OBSERVE-R001.1 The code `PARTIAL_MATERIALIZATION` is declared and carries typed detail. Files: `crates/medulla-types/src/contract/server_error_code.rs`, `crates/medulla-server-graph/src/dispatch/graph_pipeline/graph_access.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::partial_materialization_reaches_client_typed`.
- [ ] T002 [US6] MEDULLA-OBSERVE-R001.2 An uncoded refusal becomes a correlated `INTERNAL` with a cause reference. Files: `crates/medulla-protocol-types/src/protocol/support_04.rs`. Test: `crates/medulla-observe/tests/refusal_cause.rs::uncoded_refusal_logs_cause_and_returns_reference`.
- [ ] T003 [US6] MEDULLA-OBSERVE-R001 Refusals keep their type from handler to client (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::typed_refusals_reach_the_client_end_to_end`.
- [ ] T004 [US4] MEDULLA-OBSERVE-R002.1 Every log line shows its module target. Files: `crates/medulla-kernel/src/otel.rs`. Test: `crates/medulla-kernel/src/otel.rs::log_filter_tests::module_filter_matches_the_logging_module_line`.
- [ ] T005 [US6] MEDULLA-OBSERVE-R001.3 A served test fails when a refusal collapses. Files: `crates/medulla-server-served-tests/src/lib.rs`, `crates/medulla-kernel/src/metrics.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::collapsed_refusal_fails_the_harness`.
- [ ] T006 [US6] MEDULLA-OBSERVE-R001.4 The Python client surfaces code, detail and cause reference. Files: `crates/medulla-pyengine/src/errors.rs`, `medulla/client.py`. Test: `tests/test_engine_refusal_mapping.py::test_partial_materialization_and_cause_ref_reach_the_exception`.
- [ ] T007 [US4] MEDULLA-OBSERVE-R002.2 The transport reads the caller's trace context. Files: `crates/medulla-protocol-types/src/protocol.rs`, `crates/medulla-observe/src/trace.rs`, `crates/medulla-server/src/transport.rs`. Test: `crates/medulla-observe/tests/trace_context.rs::traceparent_parses_and_malformed_is_dropped`.

## Slice B: registry, state machines, introspection, readiness

- [ ] T008 [US2] MEDULLA-OBSERVE-R003.1 Registry API. Files: `crates/medulla-observe/src/registry/tasks.rs`, `crates/medulla-observe/src/lib.rs`, `Cargo.toml`. Test: `crates/medulla-observe/tests/task_registry.rs::register_progress_finish_and_drop_record_states`.
- [ ] T009 [US3] MEDULLA-OBSERVE-R004.1 The machine `TaskMachine`. Files: `crates/medulla-observe/src/machine/task.rs`. Test: `crates/medulla-observe/tests/machines.rs::task_machine_edges_t01_to_t17`.
- [ ] T010 [US1] MEDULLA-OBSERVE-R004.2 The machine `ReadinessMachine`. Files: `crates/medulla-observe/src/machine/readiness.rs`. Test: `crates/medulla-observe/tests/machines.rs::readiness_machine_edges_d01_to_d12`.
- [ ] T011 [US2] MEDULLA-OBSERVE-R004.3 The machine `RequestMachine`. Files: `crates/medulla-observe/src/machine/request.rs`. Test: `crates/medulla-observe/tests/machines.rs::request_machine_edges_q01_to_q13`.
- [ ] T012 [US3] MEDULLA-OBSERVE-R004.4 Every transition is a structured event. Files: `crates/medulla-observe/src/events.rs`. Test: `crates/medulla-observe/tests/machines.rs::accepted_and_refused_transitions_are_recorded`.

## Slice A: typed refusals, module targets, trace id

- [ ] T013 [US4] MEDULLA-OBSERVE-R002.3 A request span carries the ids on every line. Files: `crates/medulla-server-router/src/dispatch/request_boundary.rs`, `crates/medulla-observe/src/trace.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::engine_log_line_inside_request_shows_ids`.

## Slice B: registry, state machines, introspection, readiness

- [ ] T014 [US2] MEDULLA-OBSERVE-R003.2 The helper `observe::spawn` registers a job and carries the trace context. Files: `crates/medulla-observe/src/spawn.rs`, `clippy.toml`. Test: `crates/medulla-observe/tests/task_registry.rs::spawn_registers_and_panic_is_recorded_with_location`.

## Slice A: typed refusals, module targets, trace id

- [ ] T015 [US4] MEDULLA-OBSERVE-R002.4 Spawned tasks inherit the trace context. Files: `crates/medulla-observe/src/spawn.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::one_trace_id_follows_a_request_into_its_spawned_task`.
- [ ] T016 [US4] MEDULLA-OBSERVE-R002 Logs carry module targets and the caller's trace id (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::one_trace_id_follows_a_request_into_its_spawned_task`.
- [ ] T017 [US4] MEDULLA-OBSERVE-R002.5 The caller half sends the trace id (cross-repo consumer). Files: `medulla/client.py`, `specs/engine-observability/contracts/trace-correlation.md`. Test: `tests/test_trace_id_roundtrip.py::test_client_trace_id_appears_in_server_log`.
- [ ] T018 [US3] MEDULLA-OBSERVE-R011 Every observability knob is in the knob registry. Files: `crates/medulla-server-api/src/knob_config.rs`. Test: `crates/medulla-server-api/src/knob_config.rs::tests::observe_knobs_have_defaults_bounds_and_units`.

## Slice B: registry, state machines, introspection, readiness

- [ ] T019 [US3] MEDULLA-OBSERVE-R005.1 The type `ProgressGuard`. Files: `crates/medulla-observe/src/progress.rs`, `crates/medulla-types/src/contract/server_error_code.rs`. Test: `crates/medulla-observe/tests/progress.rs::equal_cursors_fail_with_name_and_location`.
- [ ] T020 [US2] MEDULLA-OBSERVE-R003.3 Lazy paging registers with a cursor. Files: `crates/medulla-persistence/src/cold_offload.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::lazy_paging_task_shows_cursor_in_introspect`.
- [ ] T021 [US2] MEDULLA-OBSERVE-R006.1 The admin read `Method::Introspect`. Files: `crates/medulla-protocol-types/src/protocol/method/method_11.rs`, `crates/medulla-capabilities/src/domains/security.rs`, `crates/medulla-capabilities/src/scopes.rs`, `crates/medulla-h-admin/src/admin/introspect.rs`, `crates/medulla-types/src/introspect.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_sections_answer_over_the_served_path`.
- [ ] T022 [US2] MEDULLA-OBSERVE-R006.2 The in-flight request registry. Files: `crates/medulla-observe/src/registry/requests.rs`, `crates/medulla-server-router/src/dispatch/request_boundary.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::in_flight_request_is_listed_with_phase_and_age`.
- [ ] T023 [US2] MEDULLA-OBSERVE-R006.4 Lock holders and waiters. Files: `crates/medulla-observe/src/registry/locks.rs`, `crates/medulla-server-api/src/routed_write_coalescer.rs`, `crates/medulla-server-graph/src/dispatch/shared.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::held_graph_lock_shows_holder_and_waiter`.
- [ ] T024 [US2] MEDULLA-OBSERVE-R006.5 Introspection exposes no payload and no secret. Files: `crates/medulla-h-admin/src/admin/introspect.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_rows_carry_no_token_or_payload`.
- [ ] T025 [US2] MEDULLA-OBSERVE-R006.6 The Python client calls it. Files: `medulla/client.py`, `medulla/generated/_ids.py`. Test: `tests/test_introspect_served.py::test_client_reads_each_section`.
- [ ] T026 [US2] MEDULLA-OBSERVE-R006.3 Runtime worker statistics. Files: `crates/medulla-observe/src/runtime_stats.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::spinning_worker_is_flagged_in_runtime_section`.
- [ ] T027 [US1] MEDULLA-OBSERVE-R007.1 Per-graph readiness is wired to open, migration and paging. Files: `crates/medulla-observe/src/registry/readiness.rs`, `crates/medulla-core/src/registry.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::graph_readiness_follows_open_migrate_materialize`.
- [ ] T028 [US1] MEDULLA-OBSERVE-R007.2 The `Health` report carries readiness and blockers. Files: `crates/medulla-protocol-types/src/result_contract/cluster/lifecycle.rs`, `crates/medulla-server-router/src/dispatch/router/service_control.rs`, `medulla/client.py`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_names_the_blocking_graph_and_reason`.
- [ ] T029 [US1] MEDULLA-OBSERVE-R007.3 The `Health` call answers within a deadline. Files: `crates/medulla-server-router/src/dispatch/router/service_control.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_answers_degraded_when_state_lock_is_held`.
- [ ] T030 [US1] MEDULLA-OBSERVE-R007.4 The probe `/readyz` serves readiness on the observability listener. Files: `crates/medulla-server-obs/src/obs/http/mod.rs`. Test: `crates/medulla-server-obs/src/obs/http/mod.rs::tests::readyz_is_503_with_blockers_until_ready`.
- [ ] T031 [US1] MEDULLA-OBSERVE-R008.1 Step records. Files: `crates/medulla-observe/src/startup.rs`. Test: `crates/medulla-observe/tests/startup.rs::step_guard_records_duration_and_outcome`.
- [ ] T032 [US1] MEDULLA-OBSERVE-R008.3 The namespace migration leaves a clear record. Files: `crates/medulla-h-admin/src/namespace_migration.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::refused_commons_migration_is_recorded_and_degrades_readiness`.
- [ ] T033 [US1] MEDULLA-OBSERVE-R008.2 The report survives a restart. Files: `crates/medulla-observe/src/startup.rs`, `crates/medulla-persistence/src/redb_backend`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::previous_boot_report_is_queryable_after_restart`.
- [ ] T034 [US1] MEDULLA-OBSERVE-R008.4 The section `Introspect startup` serves the report. Files: `crates/medulla-h-admin/src/admin/introspect.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::startup_report_lists_every_step_with_outcome`.
- [ ] T035 [US2] MEDULLA-OBSERVE-R003.4 Index builds register. Files: `crates/medulla-h-retrieve/src/semantic_index.rs`, `crates/medulla-semantic-index/src/semantic_store_ann.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::index_build_task_shows_cursor_in_introspect`.
- [ ] T036 [US2] MEDULLA-OBSERVE-R003.5 Store and namespace migrations register. Files: `crates/medulla-h-admin/src/namespace_migration.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::namespace_migration_task_and_steps_are_visible`.
- [ ] T037 [US2] MEDULLA-OBSERVE-R003.6 Boot maintenance and projection loops register. Files: `crates/medulla-h-admin/src/boot_maintenance.rs`, `crates/medulla-h-admin/src/connector_pack_projection.rs`, `crates/medulla-h-retrieve/src/reasoning_projection.rs`, `crates/medulla-server/src/replica.rs`, `crates/medulla-server/src/server_startup.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::every_inventoried_background_task_is_registered_after_boot`.
- [ ] T038 [US2] MEDULLA-OBSERVE-R003.7 The consumer seam is a written contract. Files: `specs/engine-observability/contracts/task-registry.md`. Test: `tests/docs/test_engine_observability_doc.py::test_task_registry_contract_names_every_public_item`.
- [ ] T039 [US2] MEDULLA-OBSERVE-R003 The task registry: every background job registers (rollup, PROVIDER). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::every_inventoried_background_task_is_registered_after_boot`.
- [ ] T040 [US3] MEDULLA-OBSERVE-R004.5 The machines are described in the ontology. Files: `crates/medulla-core/ontology/state-machines-v1.ttl`. Test: `crates/medulla-rdf/tests/state_machine_vocabulary.rs::observe_machines_match_edge_tables`.
- [ ] T041 [US3] MEDULLA-OBSERVE-R004.6 No writer sets state outside `apply`. Files: none. Test: `crates/medulla-observe/tests/machines.rs::no_writer_sets_state_outside_apply`.
- [ ] T042 [US3] MEDULLA-OBSERVE-R004 Tasks, graph readiness and requests are explicit state machines (rollup). Files: none. Test: `crates/medulla-observe/tests/machines.rs::every_transition_and_refusal_is_covered`.

## Slice C: forward progress in lazy paging (owned by MEDULLA-DURABLE-KERNEL-R873)

The paging rows (R873, R873.1, R873.2) are tasks T262 to T264 of the durable kernel `tasks.md`.

## Slice D: build identity and release symbols (release symbols owned by MEDULLA-CONTRACT-R060)

- [ ] T043 [US5] MEDULLA-OBSERVE-R009.1 Build identity is embedded at build time. Files: `crates/medulla-version/src/build_info.rs`, `crates/medulla-version/build.rs`. Test: `crates/medulla-version/src/build_info.rs::tests::build_info_has_version_sha_and_features`.
- [ ] T044 [US5] MEDULLA-OBSERVE-R009.2 Startup log and `Introspect build`. Files: `crates/medulla-server/src/server_startup.rs`, `crates/medulla-h-admin/src/admin/introspect.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::build_section_reports_version_sha_build_id_features`.
- [ ] T045 [US5] MEDULLA-OBSERVE-R009 The server reports which build it is (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::build_section_reports_version_sha_build_id_features`.

## Slice E: stall watchdog and profiler

- [ ] T046 [US3] MEDULLA-OBSERVE-R005.2 The stall watchdog. Files: `crates/medulla-observe/src/watchdog.rs`, `crates/medulla-server/src/server_startup.rs`. Test: `crates/medulla-observe/tests/watchdog.rs::idle_task_goes_stalled_and_recovers_on_progress`.
- [ ] T047 [US1] MEDULLA-OBSERVE-R005.3 A readiness blocker names the stalled task. Files: `crates/medulla-observe/src/watchdog.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::stalled_materialization_degrades_graph_readiness`.
- [ ] T048 [US3] MEDULLA-OBSERVE-R005 Loops prove forward progress or fail naming themselves (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::spinning_loop_is_named_and_stalled_task_is_located`.
- [ ] T049 [US5] MEDULLA-OBSERVE-R010.1 The call `Method::Profile` returns stack samples. Files: `crates/medulla-observe/src/profile.rs`, `crates/medulla-protocol-types/src/protocol/method/method_11.rs`, `crates/medulla-h-admin/src/admin/profile.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::profile_samples_the_spinning_worker`.
- [ ] T050 [US5] MEDULLA-OBSERVE-R010.2 Frames are symbolizable offline. Files: `crates/medulla-observe/src/profile.rs`. Test: `crates/medulla-observe/tests/profile.rs::frames_carry_build_id_and_offset`.
- [ ] T051 [US5] MEDULLA-OBSERVE-R010.3 The kind `TaskDump` is one atomic snapshot. Files: `crates/medulla-h-admin/src/admin/profile.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::task_dump_is_one_consistent_snapshot`.
- [ ] T052 [US5] MEDULLA-OBSERVE-R010 On-demand profiling behind admin auth (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::profile_samples_the_spinning_worker`.

## Slice B: registry, state machines, introspection, readiness

- [ ] T053 [US2] MEDULLA-OBSERVE-R006 Live introspection through one admin operation (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_sections_answer_over_the_served_path`.
- [ ] T054 [US1] MEDULLA-OBSERVE-R007 Readiness explains itself (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_names_the_blocking_graph_and_reason`.
- [ ] T055 [US1] MEDULLA-OBSERVE-R008 One structured record per post-upgrade step (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::startup_report_lists_every_step_with_outcome`.

## Slice F: chaos proof and guide

- [ ] T056 [US3] MEDULLA-OBSERVE-R012.1 Stalled task. Files: `crates/medulla-observe/src/chaos.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named`.
- [ ] T057 [US3] MEDULLA-OBSERVE-R012.2 Spinning loop. Files: `crates/medulla-observe/src/chaos.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named`.
- [ ] T058 [US3] MEDULLA-OBSERVE-R012.3 The lazy-paging incident replays. Files: `crates/medulla-observe/src/chaos.rs`. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::frozen_paging_cursor_ends_typed_and_degrades_graph`.
- [ ] T059 [US3] MEDULLA-OBSERVE-R012 Chaos proof names the culprit in bounded time (rollup). Files: none. Test: `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named`.
- [ ] T060 [US2] MEDULLA-OBSERVE-R013 A guide and a drift test cover the feature. Files: `docs/interfaces/engine-observability.md`, `tests/docs/test_engine_observability_doc.py`. Test: `tests/docs/test_engine_observability_doc.py::test_guide_matches_codes_knobs_and_sections`.

## Cross-spec rows

- The durable kernel spec holds the paging guard and the seek cursor (rows R873, R873.1, R873.2).
- The contract spec holds the release symbol artifacts and the symbolization command (rows R060, R060.1 to R060.3).
- The arbora spec holds the caller half of the trace id (consumer of R002.2).
