# MEDULLA-OBSERVE — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Served (L3) | A graph with a partial materialization manifest; a read through `dispatch_on_heap`. | The wire error is `PARTIAL_MATERIALIZATION` with the phase and cursor as JSON detail; it is never `INTERNAL`. |  |
| T-002 | FR-002 | Served (L3) | A handler returns an uncoded string. | The wire shows `INTERNAL` with a cause reference; one WARN line holds the full text and the same reference. |  |
| T-003 | FR-003 | Served (L3) | A request with a `trace_id`; it spawns a background task. | Every log line of the request and of the task shows the request id and the trace id; a `RUST_LOG=medulla::request=debug` filter matches. |  |
| T-004 | FR-004 | Unit (L1) | Each edge, each illegal pair, each terminal state of the three machines. | Legal edges fire; every other pair returns `TRANSITION_REFUSED`; each is recorded as an event. |  |
| T-005 | FR-005 | Served (L3) | Boot a served engine with the background jobs enabled; call `Introspect tasks`. | Every inventoried job is listed with owner, state and cursor. |  |
| T-006 | FR-006 | Served (L3) | A request holds the graph lock; a second waits; call `Introspect requests`, `locks`, `runtime`. | Both requests show phase and age; the lock shows holder and waiter; the busy worker is flagged. |  |
| T-007 | FR-007 | Served (L3) | A graph in Materializing with a frozen cursor; call `Health` and `GET /readyz`. | Both name the graph, the state and the blocking task within the deadline. |  |
| T-008 | FR-008 | Served (L3) | Boot twice; the first boot refuses the commons migration. | `Introspect startup` returns both reports with duration and outcome per step. |  |
| T-009 | FR-009 | Unit (L1) and Served (L3) | A guard fed equal cursors; `page_in_remaining` on a frozen cursor. | `NO_PROGRESS` names the loop and `file:line`; the graph is Degraded. |  |
| T-010 | FR-010 | Served (L3) | Call `Introspect build`; read the startup log. | Version, git SHA, build-id and features appear in both. |  |
| T-011 | FR-011 | Served (L3) | `Profile Stacks` for 2 s against a pinned worker. | Folded stacks carry the build-id, offsets and the spinning function's frames. |  |
| T-012 | FR-012 | Served (L3) | Inject a stalled task and a spinning loop. | Introspection, watchdog and readiness each name the culprit and its location within 10 s. |  |
| T-013 | FR-013 | Script (L1) | Build a release-server binary, split symbols, run the planned script `scripts/symbolize.sh`. | The offset resolves to the function; the build-id of binary and artifact match. |  |
| T-014 | FR-014 | Docs (L1) | Parse the guide. | Every code, knob and section matches the source. |  |

## Row bindings

Each row names its proving test. A test marked planned does not exist yet; its row gets a `// spec: <ID>` line when the test lands.

| Requirement | Name | Proving test |
|---|---|---|
| `MEDULLA-OBSERVE-R001` | Refusals keep their type from handler to client (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::typed_refusals_reach_the_client_end_to_end` (planned) |
| `MEDULLA-OBSERVE-R001.1` | The code `PARTIAL_MATERIALIZATION` is declared and carries typed detail | `crates/medulla-protocol-types/src/protocol/tests.rs::partial_materialization_keeps_its_code_and_json_detail` |
| `MEDULLA-OBSERVE-R001.2` | An uncoded refusal becomes a correlated `INTERNAL` with a cause reference | `crates/medulla-protocol-types/src/protocol/tests.rs::uncoded_refusal_carries_a_cause_reference_and_counts` |
| `MEDULLA-OBSERVE-R001.3` | A served test fails when a refusal collapses | `crates/medulla-server-served-tests/src/split_tests/observability.rs::collapsed_refusal_fails_the_harness` (planned) |
| `MEDULLA-OBSERVE-R001.4` | The Python client surfaces code, detail and cause reference | `tests/test_engine_refusal_mapping.py::test_partial_materialization_and_cause_ref_reach_the_exception` (planned) |
| `MEDULLA-OBSERVE-R002` | Logs carry module targets and the caller's trace id (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::one_trace_id_follows_a_request_into_its_spawned_task` (planned) |
| `MEDULLA-OBSERVE-R002.1` | Every log line shows its module target | `crates/medulla-kernel/src/otel.rs::log_filter_tests::module_filter_matches_the_logging_module_line` |
| `MEDULLA-OBSERVE-R002.2` | The transport reads the caller's trace context | `crates/medulla-observe/tests/trace_context.rs::traceparent_parses_and_malformed_is_dropped` (planned) |
| `MEDULLA-OBSERVE-R002.3` | A request span carries the ids on every line | `crates/medulla-server-served-tests/src/split_tests/observability.rs::engine_log_line_inside_request_shows_ids` (planned) |
| `MEDULLA-OBSERVE-R002.4` | Spawned tasks inherit the trace context | `crates/medulla-server-served-tests/src/split_tests/observability.rs::one_trace_id_follows_a_request_into_its_spawned_task` (planned) |
| `MEDULLA-OBSERVE-R002.5` | The caller half sends the trace id (cross-repo consumer) | `tests/test_trace_id_roundtrip.py::test_client_trace_id_appears_in_server_log` (planned) |
| `MEDULLA-OBSERVE-R003` | The task registry: every background job registers (rollup, PROVIDER) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::every_inventoried_background_task_is_registered_after_boot` (planned) |
| `MEDULLA-OBSERVE-R003.1` | Registry API | `crates/medulla-observe/tests/task_registry.rs::register_progress_finish_and_drop_record_states` (planned) |
| `MEDULLA-OBSERVE-R003.2` | The helper `observe::spawn` registers a job and carries the trace context | `crates/medulla-observe/tests/task_registry.rs::spawn_registers_and_panic_is_recorded_with_location` (planned) |
| `MEDULLA-OBSERVE-R003.3` | Lazy paging registers with a cursor | `crates/medulla-server-served-tests/src/split_tests/observability.rs::lazy_paging_task_shows_cursor_in_introspect` (planned) |
| `MEDULLA-OBSERVE-R003.4` | Index builds register | `crates/medulla-server-served-tests/src/split_tests/observability.rs::index_build_task_shows_cursor_in_introspect` (planned) |
| `MEDULLA-OBSERVE-R003.5` | Store and namespace migrations register | `crates/medulla-server-served-tests/src/split_tests/observability.rs::namespace_migration_task_and_steps_are_visible` (planned) |
| `MEDULLA-OBSERVE-R003.6` | Boot maintenance and projection loops register | `crates/medulla-server-served-tests/src/split_tests/observability.rs::every_inventoried_background_task_is_registered_after_boot` (planned) |
| `MEDULLA-OBSERVE-R003.7` | The consumer seam is a written contract | `tests/docs/test_engine_observability_doc.py::test_task_registry_contract_names_every_public_item` (planned) |
| `MEDULLA-OBSERVE-R004` | Tasks, graph readiness and requests are explicit state machines (rollup) | `crates/medulla-observe/tests/machines.rs::every_transition_and_refusal_is_covered` (planned) |
| `MEDULLA-OBSERVE-R004.1` | The machine `TaskMachine` | `crates/medulla-observe/tests/machines.rs::task_machine_edges_t01_to_t17` (planned) |
| `MEDULLA-OBSERVE-R004.2` | The machine `ReadinessMachine` | `crates/medulla-observe/tests/machines.rs::readiness_machine_edges_d01_to_d12` (planned) |
| `MEDULLA-OBSERVE-R004.3` | The machine `RequestMachine` | `crates/medulla-observe/tests/machines.rs::request_machine_edges_q01_to_q13` (planned) |
| `MEDULLA-OBSERVE-R004.4` | Every transition is a structured event | `crates/medulla-observe/tests/machines.rs::accepted_and_refused_transitions_are_recorded` (planned) |
| `MEDULLA-OBSERVE-R004.5` | The machines are described in the ontology | `crates/medulla-rdf/tests/state_machine_vocabulary.rs::observe_machines_match_edge_tables` (planned) |
| `MEDULLA-OBSERVE-R004.6` | No writer sets state outside `apply` | `crates/medulla-observe/tests/machines.rs::no_writer_sets_state_outside_apply` (planned) |
| `MEDULLA-OBSERVE-R005` | Loops prove forward progress or fail naming themselves (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::spinning_loop_is_named_and_stalled_task_is_located` (planned) |
| `MEDULLA-OBSERVE-R005.1` | The type `ProgressGuard` | `crates/medulla-observe/tests/progress.rs::equal_cursors_fail_with_name_and_location` (planned) |
| `MEDULLA-OBSERVE-R005.2` | The stall watchdog | `crates/medulla-observe/tests/watchdog.rs::idle_task_goes_stalled_and_recovers_on_progress` (planned) |
| `MEDULLA-OBSERVE-R005.3` | A readiness blocker names the stalled task | `crates/medulla-server-served-tests/src/split_tests/observability.rs::stalled_materialization_degrades_graph_readiness` (planned) |
| `MEDULLA-OBSERVE-R006` | Live introspection through one admin operation (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_sections_answer_over_the_served_path` (planned) |
| `MEDULLA-OBSERVE-R006.1` | The admin read `Method::Introspect` | `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_sections_answer_over_the_served_path` (planned) |
| `MEDULLA-OBSERVE-R006.2` | The in-flight request registry | `crates/medulla-server-served-tests/src/split_tests/observability.rs::in_flight_request_is_listed_with_phase_and_age` (planned) |
| `MEDULLA-OBSERVE-R006.3` | Runtime worker statistics | `crates/medulla-server-served-tests/src/split_tests/observability.rs::spinning_worker_is_flagged_in_runtime_section` (planned) |
| `MEDULLA-OBSERVE-R006.4` | Lock holders and waiters | `crates/medulla-server-served-tests/src/split_tests/observability.rs::held_graph_lock_shows_holder_and_waiter` (planned) |
| `MEDULLA-OBSERVE-R006.5` | Introspection exposes no payload and no secret | `crates/medulla-server-served-tests/src/split_tests/observability.rs::introspect_rows_carry_no_token_or_payload` (planned) |
| `MEDULLA-OBSERVE-R006.6` | The Python client calls it | `tests/test_introspect_served.py::test_client_reads_each_section` (planned) |
| `MEDULLA-OBSERVE-R007` | Readiness explains itself (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_names_the_blocking_graph_and_reason` (planned) |
| `MEDULLA-OBSERVE-R007.1` | Per-graph readiness is wired to open, migration and paging | `crates/medulla-server-served-tests/src/split_tests/observability.rs::graph_readiness_follows_open_migrate_materialize` (planned) |
| `MEDULLA-OBSERVE-R007.2` | The `Health` report carries readiness and blockers | `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_names_the_blocking_graph_and_reason` (planned) |
| `MEDULLA-OBSERVE-R007.3` | The `Health` call answers within a deadline | `crates/medulla-server-served-tests/src/split_tests/observability.rs::health_answers_degraded_when_state_lock_is_held` (planned) |
| `MEDULLA-OBSERVE-R007.4` | The probe `/readyz` serves readiness on the observability listener | `crates/medulla-server-obs/src/obs/http/mod.rs::tests::readyz_is_503_with_blockers_until_ready` (planned) |
| `MEDULLA-OBSERVE-R008` | One structured record per post-upgrade step (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::startup_report_lists_every_step_with_outcome` (planned) |
| `MEDULLA-OBSERVE-R008.1` | Step records | `crates/medulla-observe/tests/startup.rs::step_guard_records_duration_and_outcome` (planned) |
| `MEDULLA-OBSERVE-R008.2` | The report survives a restart | `crates/medulla-server-served-tests/src/split_tests/observability.rs::previous_boot_report_is_queryable_after_restart` (planned) |
| `MEDULLA-OBSERVE-R008.3` | The namespace migration leaves a clear record | `crates/medulla-server-served-tests/src/split_tests/observability.rs::refused_commons_migration_is_recorded_and_degrades_readiness` (planned) |
| `MEDULLA-OBSERVE-R008.4` | The section `Introspect startup` serves the report | `crates/medulla-server-served-tests/src/split_tests/observability.rs::startup_report_lists_every_step_with_outcome` (planned) |
| `MEDULLA-OBSERVE-R009` | The server reports which build it is (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::build_section_reports_version_sha_build_id_features` (planned) |
| `MEDULLA-OBSERVE-R009.1` | Build identity is embedded at build time | `crates/medulla-version/src/build_info.rs::tests::build_info_has_version_sha_and_features` (planned) |
| `MEDULLA-OBSERVE-R009.2` | Startup log and `Introspect build` | `crates/medulla-server-served-tests/src/split_tests/observability.rs::build_section_reports_version_sha_build_id_features` (planned) |
| `MEDULLA-OBSERVE-R010` | On-demand profiling behind admin auth (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::profile_samples_the_spinning_worker` (planned) |
| `MEDULLA-OBSERVE-R010.1` | The call `Method::Profile` returns stack samples | `crates/medulla-server-served-tests/src/split_tests/observability.rs::profile_samples_the_spinning_worker` (planned) |
| `MEDULLA-OBSERVE-R010.2` | Frames are symbolizable offline | `crates/medulla-observe/tests/profile.rs::frames_carry_build_id_and_offset` (planned) |
| `MEDULLA-OBSERVE-R010.3` | The kind `TaskDump` is one atomic snapshot | `crates/medulla-server-served-tests/src/split_tests/observability.rs::task_dump_is_one_consistent_snapshot` (planned) |
| `MEDULLA-OBSERVE-R011` | Every observability knob is in the knob registry | `crates/medulla-server-api/src/knob_config.rs::tests::observe_knobs_have_defaults_bounds_and_units` (planned) |
| `MEDULLA-OBSERVE-R012` | Chaos proof names the culprit in bounded time (rollup) | `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named` (planned) |
| `MEDULLA-OBSERVE-R012.1` | Stalled task | `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named` (planned) |
| `MEDULLA-OBSERVE-R012.2` | Spinning loop | `crates/medulla-server-served-tests/src/split_tests/observability.rs::chaos_stalled_task_and_spinning_loop_are_named` (planned) |
| `MEDULLA-OBSERVE-R012.3` | The lazy-paging incident replays | `crates/medulla-server-served-tests/src/split_tests/observability.rs::frozen_paging_cursor_ends_typed_and_degrades_graph` (planned) |
| `MEDULLA-OBSERVE-R013` | A guide and a drift test cover the feature | `tests/docs/test_engine_observability_doc.py::test_guide_matches_codes_knobs_and_sections` (planned) |
