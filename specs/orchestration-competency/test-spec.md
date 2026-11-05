# MEDULLA-CQ-ORCH — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Unit | the fixture graph, shape and query of each of questions 1 to 7 | a non-empty provenance-carrying answer; a planted-invalid shape input is rejected | `crates/medulla-rdf/tests/orchestration_competency.rs` |
| T-002 | FR-001 | Negative | a source that is not connected | the typed abstention `Uncovered`, never an empty success | `crates/medulla-rdf/tests/orchestration_competency.rs` |
| T-003 | FR-002 | Served | task XYZ imported through `ConnectorPack.Import`, asked through `AgentExecutionAnswer` | seven parts with premises, `assembled_by` is `graph.assemble` | PENDING |
| T-004 | FR-003 | Served | the six probe questions against the engine without arbora | the documented answer with provenance for each probe | PENDING |
| T-005 | FR-004 | Served | a fixture repository and one diagram | containers with support paths; an empty scope abstains | PENDING |
| T-006 | FR-005 | Served | two imported connector packs, one downtime window, one plan evolution | five answers with provenance; cross-tenant asks return empty answers | PENDING |
| T-007 | FR-006 | Served | a process of 12 nodes compiled and an EPC fixture imported and exported | versioned workflow, replayable digest, gates preserved | PENDING |

## Planned tests per row

Every row below names a test that does not exist yet. The test file and function are fixed by the row; no case has run.

| Test ID | Row | Level | Planned test | Expected observation |
|---|---|---|---|---|
| T-008 | MEDULLA-CQ-ORCH-R008.2 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_envelope_assembled_by_graph_assemble` | Composed assemble wiring proves its row as stated in requirements.md |
| T-009 | MEDULLA-CQ-ORCH-R008.3 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_missing_source_abstains_one_part` | Composed typed abstention per part proves its row as stated in requirements.md |
| T-010 | MEDULLA-CQ-ORCH-R008.4 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_fixture_ingest_produces_expected_part_rows` | Composed ingest fixture proves its row as stated in requirements.md |
| T-011 | MEDULLA-CQ-ORCH-R008.5 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_composed_answer_has_seven_parts_with_provenance` | Composed acceptance test proves its row as stated in requirements.md |
| T-012 | MEDULLA-CQ-ORCH-R009.1 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_1` | Probe 1: node counts by label proves its row as stated in requirements.md |
| T-013 | MEDULLA-CQ-ORCH-R009.2 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_2` | Probe 2: deploy a container proves its row as stated in requirements.md |
| T-014 | MEDULLA-CQ-ORCH-R009.3 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_3` | Probe 3: agents in the Agent Library proves its row as stated in requirements.md |
| T-015 | MEDULLA-CQ-ORCH-R009.4 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_4` | Probe 4: why the intent surface proves its row as stated in requirements.md |
| T-016 | MEDULLA-CQ-ORCH-R009.5 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_5` | Probe 5: find a container-listing tool proves its row as stated in requirements.md |
| T-017 | MEDULLA-CQ-ORCH-R009.6 | L4 | `tests/orchestration/test_probe_answers_served.py::test_probe_6` | Probe 6: delegate listing containers proves its row as stated in requirements.md |
| T-018 | MEDULLA-CQ-ORCH-R010.1 | L1 | `crates/medulla-core/tests/architecture_view.rs::view_types_round_trip` | Architecture view model proves its row as stated in requirements.md |
| T-019 | MEDULLA-CQ-ORCH-R010.2 | L1 | `crates/medulla-core/tests/architecture_view.rs::derivation_merges_code_and_diagram_elements` | Architecture view derivation proves its row as stated in requirements.md |
| T-020 | MEDULLA-CQ-ORCH-R010.3 | L3 | `tests/architecture_view_served.rs::served_operation_returns_view` | Architecture view wiring proves its row as stated in requirements.md |
| T-020a | MEDULLA-CQ-ORCH-R010.3 | L3 | `tests/architecture_view_served.rs::stale_revision_compares_latest_committed_snapshot` | the first digest is refused as stale and the digest of the latest committed snapshot returns the view |
| T-021 | MEDULLA-CQ-ORCH-R010.4 | L1 | `crates/medulla-core/tests/architecture_view.rs::empty_scope_abstains` | Architecture view refusal proves its row as stated in requirements.md |
| T-022 | MEDULLA-CQ-ORCH-R010.5 | L1 | `crates/medulla-core/tests/architecture_view.rs::every_container_cites_support` | Architecture view evidence proves its row as stated in requirements.md |
| T-023 | MEDULLA-CQ-ORCH-R010.6 | L3 | `tests/architecture_view_served.rs::served_view_over_fixture_repository` | Architecture view hot-path proof proves its row as stated in requirements.md |
| T-024 | MEDULLA-CQ-ORCH-R011.1 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_load_set` | Job load-set question proves its row as stated in requirements.md |
| T-025 | MEDULLA-CQ-ORCH-R011.2 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_downtime_next` | Downtime next-job question proves its row as stated in requirements.md |
| T-026 | MEDULLA-CQ-ORCH-R011.3 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_best_plan` | Best virtual query plan question proves its row as stated in requirements.md |
| T-027 | MEDULLA-CQ-ORCH-R011.4 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_usage_guidance` | Connector usage guidance question proves its row as stated in requirements.md |
| T-028 | MEDULLA-CQ-ORCH-R011.5 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_seed_changes` | Connector seed changes question proves its row as stated in requirements.md |
| T-029 | MEDULLA-CQ-ORCH-R011.6 | L4 | `tests/orchestration/test_orchestration_competency_served.py::test_q11_all_five_questions_after_connector_import` | Job, downtime and plan hot-path proof proves its row as stated in requirements.md |
| T-030 | MEDULLA-CQ-ORCH-R700.2 | L1 | `crates/medulla-core/tests/process_compile.rs::compile_is_linear_and_deterministic` | Process compile behaviour proves its row as stated in requirements.md |
| T-031 | MEDULLA-CQ-ORCH-R700.3 | L3 | `tests/process_compile_served.rs::second_compile_writes_next_version_and_cas_refuses_stale` | Process compile wiring and version store proves its row as stated in requirements.md |
| T-032 | MEDULLA-CQ-ORCH-R700.4 | L3 | `tests/process_compile_served.rs::compile_refusals_leave_state_unchanged` | Process compile refusal proves its row as stated in requirements.md |
| T-033 | MEDULLA-CQ-ORCH-R700.5 | L3 | `tests/process_compile_served.rs::replay_reproduces_recorded_digest` | Process compile evidence proves its row as stated in requirements.md |
| T-034 | MEDULLA-CQ-ORCH-R700.6 | L4 | `tests/competency/test_process_compile_served.py::test_cycle_without_gate_refused` | Process compile hot-path proof proves its row as stated in requirements.md |
| T-035 | MEDULLA-CQ-ORCH-R701.1 | L1 | `crates/medulla-types/src/process_import.rs::tests::imported_process_round_trips` | Imported process model and gate classification proves its row as stated in requirements.md |
| T-036 | MEDULLA-CQ-ORCH-R701.2 | L1 | `crates/medulla-core/tests/process_import.rs::import_export_round_trip_is_lossless` | Process model import and export behaviour proves its row as stated in requirements.md |
| T-037 | MEDULLA-CQ-ORCH-R701.3 | L3 | `tests/process_import_served.rs::import_stores_versioned_workflow_and_export_reads_it` | Process model import and export wiring proves its row as stated in requirements.md |
| T-038 | MEDULLA-CQ-ORCH-R701.4 | L3 | `tests/process_import_served.rs::import_refusals_leave_state_unchanged` | Process model import refusal proves its row as stated in requirements.md |
| T-039 | MEDULLA-CQ-ORCH-R701.5 | L3 | `tests/process_import_served.rs::stored_steps_keep_source_ids` | Process model import evidence proves its row as stated in requirements.md |
| T-040 | MEDULLA-CQ-ORCH-R701.6 | L4 | `tests/competency/test_governance_roundtrip_served.py::test_roundtrip_preserves_gates` | Process model import hot-path proof proves its row as stated in requirements.md |
| T-041 | MEDULLA-CQ-ORCH-R012 | L3 | `tests/orchestration/test_orchestration_competency_served.py::test_execute_task_names_onboarded_operations_with_provenance` | Served proof of the execution question from onboarded connector operations proves its row as stated in requirements.md |

## Negative and boundary cases

A source that is not connected: typed abstention naming the source. A planted-invalid shape input: rejected. A part of the composed answer without data: a typed abstention for that part only, never a dropped field. Cross-tenant requests: empty answers or `ACCESS_DENIED` before any read. A refusal of the compiler, the importer or the architecture view changes no durable state.

## Quality and release proof

Focused: `cargo test -p medulla-rdf --test orchestration_competency --test orchestration_competency_r0061 --test orchestration_competency_r0071` and `cargo test -p medulla-decision-outcome-types execution_answer`. Gates: the repository complexity, duplication, dead-code and KISS gates and the spec gate set. Record revision, environment, output link and pass or fail in the acceptance record; a source test is not served acceptance.
