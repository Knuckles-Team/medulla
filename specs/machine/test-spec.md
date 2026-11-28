# MEDULLA-MACHINE — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | L2 | Load every core ontology file and every Turtle fixture | The six retired IRIs are absent. `core:Machine` is declared once and registered | |
| T-002 | FR-002 | L1 | The seven vectors of `data-model.md` | Each IRI equals the recorded value. Hostname IRIs differ across tenants | |
| T-003 | FR-003 | L2 | One machine with seven part kinds, then one invalid part per shape | The valid machine passes. The shape refuses each invalid part | |
| T-004 | FR-004 | L2 | Three connector records that observe one machine | One machine answers with three observers. A non-machine target is refused | |
| T-005 | FR-005 | L1 | All 25 pairs of state and event | Seven pairs pass. 18 return `TRANSITION_REFUSED`. The Turtle edge set equals the table | |
| T-006 | FR-006 | L1 | One hosts file in two orderings, then one invalid file per reason | Both orderings give identical bytes. Each invalid file refuses with its reason and no echoed value | |
| T-007 | FR-007 | L1 | The vectors and refusals through the Python module | Python results equal the Rust results | |
| T-008 | FR-008 | L3 | The fixture record set through `SourceIngest`, then a `UnifiedQuery` | One machine in state `active` with three observers | |

## Row proofs

- `MEDULLA-MACHINE-R001`: `crates/medulla-core/tests/machine_vocab.rs::machine_is_the_only_machine_class`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R001.1`: `crates/medulla-core/tests/machine_vocab.rs::machine_class_is_declared_once_and_registered`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R001.2`: `crates/medulla-core/tests/machine_vocab.rs::no_retired_machine_class_remains`. The test scans every `*.ttl` under `crates/medulla-core/` and `crates/medulla-rdf/tests/` for the six retired IRIs and finds none. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R001.3`: `crates/medulla-core/tests/machine_vocab.rs::server_is_a_software_server_and_core_comments_name_no_product`. The test reads the `:Server` comment and scans every class comment of `infrastructure-v1.ttl` and `machine-v1.ttl` for the names `Home-Assistant`, `tunnel-manager`, `Kubernetes`, `Swarm`, `Docker`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R001.4`: `crates/medulla-stream/src/telemetry/tests/binding.rs::host_name_binds_to_a_machine`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R002`: `crates/medulla-source-types/src/machine/identity.rs::tests::every_vector_returns_its_recorded_iri`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R002.1`: `crates/medulla-source-types/src/machine/identity.rs::tests::normalization_and_placeholder_refusals_match_the_table`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R002.2`: `crates/medulla-source-types/src/machine/identity.rs::tests::every_vector_returns_its_recorded_iri`, `::select_key_prefers_uuid_then_serial_then_bmc_then_host`, `::hostname_iri_differs_across_tenants`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R002.3`: `crates/medulla-core/tests/machine_vocab.rs::machine_shape_accepts_a_valid_machine_and_refuses_each_violation`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R003`: `crates/medulla-core/tests/machine_vocab.rs::machine_with_all_seven_part_kinds_validates`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R003.1`: `crates/medulla-core/tests/machine_vocab.rs::every_part_term_is_declared_with_label_comment_and_range`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R003.2`: `crates/medulla-core/tests/machine_vocab.rs::part_shapes_accept_valid_parts_and_refuse_each_violation`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R003.3`: `crates/medulla-core/tests/machine_vocab.rs::no_retired_machine_class_remains`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R004`: `crates/medulla-rdf/tests/machine_competency.rs::three_connector_records_answer_one_machine_question`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R004.1`: `crates/medulla-core/tests/machine_vocab.rs::observes_accepts_a_machine_and_refuses_any_other_target`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R004.2`: `crates/medulla-core/tests/machine_vocab.rs::probe_snapshot_shape_requires_digest_and_exactly_one_machine`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005`: `crates/medulla-rdf/tests/state_machine_vocabulary.rs::machine_lifecycle_machine_matches_edge_table`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005.1`: `crates/medulla-source-types/src/machine/lifecycle.rs::tests::edge_h01_commissioned_ready` to `::edge_h07_commissioned_retire`, one test per edge. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005.2`: `crates/medulla-source-types/src/machine/lifecycle.rs::tests::derive_event_table_matches_the_row` and `::guards_of_one_state_are_mutually_exclusive_over_all_64_fact_sets`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005.3`: `crates/medulla-source-types/src/machine/lifecycle.rs::tests::every_illegal_pair_is_refused` (25 pairs, 7 legal, 18 refused) and `::no_writer_sets_state_outside_apply` (a source scan of `crates/` finds no other assignment of the stored text). (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005.4`: `crates/medulla-rdf/tests/state_machine_vocabulary.rs::machine_lifecycle_machine_matches_edge_table` and `::machine_lifecycle_machine_classes_extend_core`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R005.5`: `crates/medulla-core/tests/machine_vocab.rs::transition_record_shape_refuses_an_unknown_edge_and_a_missing_machine`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R006`: `tests/test_machine_inventory_ingest_served.py::test_records_reconcile_into_the_tenant_graph`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R006.1`: `crates/medulla-source-types/src/machine/hosts.rs::tests::each_refusal_reason_has_a_fixture_and_never_echoes_the_value` and `::hosts_file_schema_lists_exactly_the_documented_fields`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R006.2`: `crates/medulla-source-types/src/machine/hosts.rs::tests::two_orderings_of_one_file_normalize_to_identical_bytes`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R006.3`: `crates/medulla-source-types/src/machine/records.rs::tests::canonical_json_is_byte_stable_under_input_reordering` and `::ids_match_the_recorded_vectors`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R006.4`: `tests/test_machine_inventory_ingest_served.py::test_records_reconcile_into_the_tenant_graph` (served) and `::test_a_machine_that_fails_the_shape_refuses_the_batch`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R007`: `crates/medulla-pyengine/tests/test_machine_functions.py::test_every_vector_and_refusal_matches_the_rust_functions`. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R008`: `tests/test_machine_wiring.py::test_every_new_symbol_has_a_production_caller`. The check searches each new public function for a caller outside tests. (planned; the owning change adds the files)
- `MEDULLA-MACHINE-R009`: `tests/test_machine_inventory_ingest_served.py::test_three_connector_records_resolve_to_one_machine_with_its_state` (served), and `crates/medulla-rdf/tests/machine_competency.rs::three_connector_records_answer_one_machine_question` (L2). (planned; the owning change adds the files)

## Negative and boundary cases

- A placeholder UUID, a placeholder serial and an all-zero controller ID each count as absent.
- A hosts file with a `password` key at any depth refuses and never echoes the value.
- An Active machine refuses `Retire`. A Retired machine refuses every event with reason `terminal_state`.
- A reconcile batch that omits a machine removes it. A batch that carries an invalid machine changes nothing.
- A hostname key gives different IRIs for two tenants.

## Quality and release proof

- Focused: `cargo nextest run -p medulla-source-types machine` and `cargo nextest run -p medulla-core machine_vocab` and `cargo nextest run -p medulla-rdf state_machine_vocabulary machine_competency`.
- Wheel: `maturin develop` for `crates/medulla-pyengine`, then `pytest crates/medulla-pyengine/tests/test_machine_functions.py`.
- Served: `pytest tests/test_machine_inventory_ingest_served.py`.
- Gates: `cargo clippy -- -D warnings`, `scripts/check_crate_graph.py`, the state-machine-shape hook, `pipelines-hook spec-standard`.
