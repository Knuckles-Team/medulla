# MEDULLA-REPO-INGEST — Test specification

Tests are keyed to the published commit. Each fixture uses a temporary Medulla store and public synthetic source. No private repository, inventory, credential or network service is a prerequisite. A source test proves implementation only; acceptance also needs the served path where a requirement crosses a process or replica.

## Proof levels

| Level | Harness |
|---|---|
| L1 Unit | A `#[cfg(test)]` module or a crate test |
| L2 Property and golden vector | Property tests, golden vectors and exhaustive small cases |
| L3 Served | The served harness or a served Python test through the production dispatch |
| L4 Served client | A generated-client test over a loopback engine with authentication |
| L5 Nightly bench | A criterion bench with a p50, p99 and memory budget |

## Per-row proof map

One row per leaf requirement. A planned test names the exact path that the row adds.

| ID | Test | Level | Required observation |
|---|---|---|---|
| `MEDULLA-REPO-INGEST-R001` | `crates/medulla-ann-codes/src/semantic_index_service/reconciliation_regression.rs::reconcile_readmits_stranded_intent_after_originating_topic_retires` | L3 | An integration test retires a topic with pending stranded intents, then runs a reconcile-mode SourceIngest pass. The stranded intents must be re-admitted and committed. |
| `MEDULLA-REPO-INGEST-R004` | `crates/medulla-tsdb/src/derive/maintain_tests.rs::the_work_budget_stops_a_step_and_the_next_continues` | L3 | An integration test defines a derived series, appends to its sources, confirms incremental maintenance matches full recomputation, and negative tests confirm a cyclic definition is refused and an over-budget maintenance run is bounded. |
| `MEDULLA-REPO-INGEST-R005` | `crates/medulla-server-api/src/request_replay.rs::an_accepted_nonce_is_refused_after_a_reopen` | L2 | A benchmark compares served throughput before/after at matching concurrency. A replay-attack negative test continues to pass. |
| `MEDULLA-REPO-INGEST-R007` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_repository_index_tests.rs::a_batch_over_the_commit_budget_is_refused_whole` | L3 | A regression test submits a batch past the configured limit and confirms the atomic refusal, the stable error code, and that no partial graph mutation occurred. |
| `MEDULLA-REPO-INGEST-R008` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_repository_index_tests.rs::a_batch_over_the_byte_ceiling_is_refused_and_a_halved_resend_succeeds` | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R009.1` | `crates/medulla-graph-core/src/graph/core_edge_queries.rs::typed_edges_are_addressable_by_relation_type_triple` | L1 | A collision test adds two untyped edges between the same pair that migrate onto the same `relation_type`, confirms the deterministic collapse and its collision receipt, and a typed-edge test confirms distinct relation types between the same pair are independently addressable, retrievable and removable. Test: `crates/medulla-graph-core/src/graph/core_edge_queries.rs::typed_edges |
| `MEDULLA-REPO-INGEST-R009.2` | `crates/medulla-h-ingest/src/repository_index.rs::two_relations_between_the_same_endpoints_stay_distinct_edges` | L3 | It queries each edge by its `relation_type`. |
| `MEDULLA-REPO-INGEST-R010` | `crates/medulla-compute/tests/grammar_expansion.rs::cuda_cpp_fixture_preserves_cuda_label` | L2 | A grammar-expansion test suite parses a representative fixture for each newly added language. Every fixture must report a successful parse. |
| `MEDULLA-REPO-INGEST-R011` | `crates/medulla-graph-algos/src/algorithms/tests.rs::every_community_induces_a_connected_subgraph` | L2 | A test confirms the ingestion path calls Leiden refinement. No committed community in the test graph is internally disconnected. |
| `MEDULLA-REPO-INGEST-R002.1.1` | `crates/medulla-ingestion-types/tests/skill_plan.rs::compilation_round_trips` (planned) | L1 | Verified by the root slice test `tests/test_skill_plan_served.py::test_skill_document_alone_compiles_and_binds`. |
| `MEDULLA-REPO-INGEST-R002.1.2` | `crates/medulla-document/tests/skill_plan.rs::steps_compile_in_source_order_with_dependencies` (planned) | L2 | The named test passes. |
| `MEDULLA-REPO-INGEST-R002.1.3` | `tests/test_skill_plan_served.py::test_changed_document_marks_binding_stale` (planned) | L3 | It mutates the document after compilation and asserts the stale refusal. |
| `MEDULLA-REPO-INGEST-R002.1.4` | `tests/test_skill_plan_served.py::test_skill_plan_refusals` (planned) | L3 | It asserts each code and zero new nodes. |
| `MEDULLA-REPO-INGEST-R002.1.5` | `tests/test_skill_plan_served.py::test_skill_compilation_reads_back_without_document_copy` (planned) | L3 | It asserts no node holds the document text. |
| `MEDULLA-REPO-INGEST-R006.1` | `tests/test_ingest_throughput_served.py::test_window_counts_distinct_papers_and_reports_unknown` (planned) | L3 | It counts 1999 then 2000 distinct papers, duplicates, a partial window and stale telemetry. |
| `MEDULLA-REPO-INGEST-R006.2` | `tests/test_reembed_importance_served.py::test_high_importance_moves_less_and_gate_precedes_tag` (planned) | L3 | It also asserts zero embedding calls on the query path. Exact math: `crates/medulla-ann-codes/src/semantic_index_service/importance.rs::squared_l2_anchor_matches_closed_form` (L1). |
| `MEDULLA-REPO-INGEST-R006.3` | `tests/test_code_map_served.py::test_current_map_locates_behavior_and_refuses_old_generation` (planned) | L3 | It changes one symbol and asserts `CODE_MAP_STALE` until reindex. It then asserts the new span. It asserts the unchanged logical id of the other symbol. |
| `MEDULLA-REPO-INGEST-R006.4` | `tests/test_incremental_index_served.py::test_delta_applies_and_releases_memory` (planned) | L3 | It runs add, modify, delete and rename, counts parser calls and asserts release after cancel. |
| `MEDULLA-REPO-INGEST-R002.100` | `tests/test_ingest_readback_served.py::test_matched_readback_and_each_refusal` | L3 | It asserts `Matched`, then `READBACK_STALE`, `READBACK_DENIED`, `READBACK_PARTIAL` and `READBACK_MISMATCH`, and that refusals write nothing. `crates/medulla-h-ingest/src/ingest_readback.rs::tests::matched_then_each_refusal` proves the same ordering on the pure comparison (L2). |
| `MEDULLA-REPO-INGEST-R002.101` | `tests/test_ingest_readback_served.py::test_hydration_run_binds_workflow_and_readback` (planned) | L3 | Binds a matching claim and reads the `:HydrationRun` node back; a wrong `workflow_revision` or `plan_digest` refuses with `HYDRATION_BINDING_MISMATCH` and writes no node. Also `crates/medulla-h-ingest/src/ingest_readback.rs::tests::hydration_binding_refuses_stale_revision_and_plan_mismatch` (L1). |
| `MEDULLA-REPO-INGEST-R002.102` | `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs::a_second_batch_to_the_same_stream_chains_its_content_version` | L3 | Served test `a_second_batch_to_the_same_stream_chains_its_content_version` in `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs`. It ingests two delta batches to one stream and asserts both commit. It asserts the stream's stored content version holds the second batch digest, chained from the first. Test: `crates/medulla-h-admin/src/admin/connector_pack/tests/scale |
| `MEDULLA-REPO-INGEST-R600.1` | `crates/medulla-enrich/src/lib.rs::identifiers_are_validated_at_construction_and_on_the_wire` | L1 | Unit test round-trips each type and rejects an invalid identifier. |
| `MEDULLA-REPO-INGEST-R600.2` | `crates/medulla-enrich/src/code.rs::rules_cover_rungs_patterns_features_and_test_quality` | L1 | Unit tests cover each rule with positive and boundary cases. |
| `MEDULLA-REPO-INGEST-R600.3` | `tests/test_repository_enrich_served.py::test_enrich_runs_once_after_index_commit` (planned) | L3 | It asserts one enrichment envelope per index envelope, produced by the claimed follow-on work item with inputs read from the committed index graph by digest, and no second caller. |
| `MEDULLA-REPO-INGEST-R600.4` | `crates/medulla-enrich/src/code.rs::a_lower_rung_overwrite_is_refused_counted_and_not_applied` | L1 | Test submits the invalid input and asserts the code and zero partial writes. |
| `MEDULLA-REPO-INGEST-R600.5` | `crates/medulla-enrich/src/code.rs::the_run_report_counts_success_and_refusal` | L1 | Test reads the trace or metrics after one success and one refusal. |
| `MEDULLA-REPO-INGEST-R600.6` | `crates/medulla-h-ingest/benches/r600_enrich_served.rs::enrich_served` (planned) | L5 | Test: nightly bench group `repo_index/enrich` (L5). The served test of `R600.3` gates the PR. |
| `MEDULLA-REPO-INGEST-R601.1` | `crates/medulla-enrich/src/history.rs::history_types_round_trip_and_invalid_identifiers_are_refused` | L1 | Unit test round-trips each type and rejects an invalid identifier. |
| `MEDULLA-REPO-INGEST-R601.2` | `crates/medulla-enrich/src/history.rs::commits_apply_once_churn_aggregates_and_coupling_respects_floors` | L1 | Unit tests cover each rule with positive and boundary cases. |
| `MEDULLA-REPO-INGEST-R601.3` | `tests/test_repository_enrich_served.py::test_second_hydration_skips_stored_commits` (planned) | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R601.4` | `crates/medulla-enrich/src/history.rs::a_history_above_the_bound_is_truncated_at_a_commit_boundary` | L1 | Test submits the invalid input and asserts the code and zero partial writes. |
| `MEDULLA-REPO-INGEST-R601.5` | `crates/medulla-enrich/src/history.rs::the_run_report_counts_added_skipped_and_coupling` | L1 | Test reads the trace or metrics after one success and one refusal. |
| `MEDULLA-REPO-INGEST-R601.6` | `crates/medulla-h-ingest/benches/r601_history_served.rs::history_served` | L5 | Test: nightly bench group `repo_index/history` (L5). |
| `MEDULLA-REPO-INGEST-R602.1` | `crates/medulla-enrich/src/links.rs::link_types_round_trip_and_invalid_identifiers_are_refused` | L1 | Unit test round-trips each type and rejects an invalid identifier. |
| `MEDULLA-REPO-INGEST-R602.2` | `crates/medulla-enrich/src/links.rs::resources_and_routes_link_to_the_longest_owner_and_features_resolve_above_the_floor` | L1 | Unit tests cover each rule with positive and boundary cases. |
| `MEDULLA-REPO-INGEST-R602.3` | `tests/test_repository_enrich_served.py::test_link_pass_runs_after_features` | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R602.4` | `crates/medulla-enrich/src/links.rs::a_tie_between_two_services_writes_no_link` | L1 | Test submits the invalid input and asserts the code and zero partial writes. |
| `MEDULLA-REPO-INGEST-R602.5` | `crates/medulla-enrich/src/links.rs::the_run_report_counts_links_and_ties` | L1 | Test reads the trace or metrics after one success and one refusal. |
| `MEDULLA-REPO-INGEST-R602.6` | `crates/medulla-h-ingest/benches/r602_links_served.rs::links_served` | L5 | Test: nightly bench group `repo_index/links` (L5). |
| `MEDULLA-REPO-INGEST-R603.1` | `crates/medulla-enrich/src/fingerprint.rs::fingerprint_types_round_trip_and_invalid_identifiers_are_refused` | L1 | Unit test round-trips each type and rejects an invalid identifier. |
| `MEDULLA-REPO-INGEST-R603.2` | `crates/medulla-enrich/src/fingerprint.rs::levels_blast_radius_and_code_map_follow_the_rules` | L1 | Unit tests cover each rule with positive and boundary cases. |
| `MEDULLA-REPO-INGEST-R603.3` | `tests/test_repository_enrich_served.py::test_cosmetic_change_skips_enrichment` (planned) | L3 | After a whitespace-only body edit in file A and no change in file B, the second `RepositoryEnrichmentRun` node reads `fingerprint_cosmetic` 1, `fingerprint_none` 1, `fingerprint_structural` 0 and `entities` 0. Also `crates/medulla-h-ingest/src/repository_index/enrich/fingerprint.rs::tests::a_body_edit_is_cosmetic_and_a_signature_edit_is_structural` (L1). |
| `MEDULLA-REPO-INGEST-R603.4` | `crates/medulla-enrich/src/fingerprint.rs::an_unparseable_file_is_structural_and_never_skipped` | L1 | Test submits the invalid input and asserts the code and zero partial writes. |
| `MEDULLA-REPO-INGEST-R603.5` | `crates/medulla-enrich/src/fingerprint.rs::the_run_report_counts_levels_and_low_usage_symbols` | L1 | Test reads the trace or metrics after one success and one refusal. |
| `MEDULLA-REPO-INGEST-R603.6` | `crates/medulla-h-ingest/benches/r603_fingerprint_served.rs::fingerprint_served` (planned) | L5 | Test: nightly bench group `repo_index/fingerprint` (L5). |
| `MEDULLA-REPO-INGEST-R800.1` | `crates/medulla-document/src/transcript.rs::dialects_map_to_one_turn_record` | L1 | Test: `crates/medulla-document/src/transcript.rs::dialects_map_to_one_turn_record`. |
| `MEDULLA-REPO-INGEST-R800.2` | RETIRED: the engine-side line reader and file checkpoint have no production caller. | - | Retired; see `MEDULLA-REPO-INGEST-R800.3`. |
| `MEDULLA-REPO-INGEST-R800.3` | RETIRED: an engine-side scheduled transcript reader is a second ingest route with no consumer. | - | Retired; see `MEDULLA-INGEST-WORKER-R003.15`. |
| `MEDULLA-REPO-INGEST-R800.4` | RETIRED: depends only on the retired engine-side transcript reader. | - | Retired; see `MEDULLA-REPO-INGEST-R800.3`. |
| `MEDULLA-REPO-INGEST-R800.5` | RETIRED: depends only on the retired engine-side transcript reader. | - | Retired; see `MEDULLA-REPO-INGEST-R800.3`. |
| `MEDULLA-REPO-INGEST-R800.6` | RETIRED: depends only on the retired engine-side transcript reader. | - | Retired; see `MEDULLA-REPO-INGEST-R800.3`. |
| `MEDULLA-REPO-INGEST-R650.1` | RETIRED: the hydration types have no production consumer. | - | Retired; see `MEDULLA-REPO-INGEST-R650.3`. |
| `MEDULLA-REPO-INGEST-R650.2` | RETIRED: the hydration planner has no production consumer. | - | Retired; see `MEDULLA-REPO-INGEST-R650.3`. |
| `MEDULLA-REPO-INGEST-R650.3` | RETIRED: engine-side boot hydration would be a second producer of index and pack batches. | - | Retired; see `MEDULLA-REPO-INGEST-R006.4`. |
| `MEDULLA-REPO-INGEST-R650.4` | RETIRED: depends only on the retired boot hydration. | - | Retired; see `MEDULLA-REPO-INGEST-R650.3`. |
| `MEDULLA-REPO-INGEST-R650.5` | RETIRED: depends only on the retired boot hydration. | - | Retired; see `MEDULLA-REPO-INGEST-R650.3`. |
| `MEDULLA-REPO-INGEST-R650.6` | RETIRED: depends only on the retired boot hydration. | - | Retired; see `MEDULLA-REPO-INGEST-R650.3`. |
| `MEDULLA-REPO-INGEST-R651.1` | RETIRED: adapter types with no production consumer. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R651.2` | RETIRED: depends only on the retired adapters. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R651.3` | RETIRED: depends only on the retired adapters. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R651.4` | RETIRED: depends only on the retired adapters. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R651.5` | RETIRED: depends only on the retired adapters. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R651.6` | RETIRED: depends only on the retired adapters. | - | Retired; see `MEDULLA-REPO-INGEST-R651`. |
| `MEDULLA-REPO-INGEST-R652.1` | `crates/medulla-document/tests/processing.rs::types_round_trip` (planned) | L1 | Verified by the root slice test `tests/web_document_enrich_served.rs::section_tree_and_links_are_written`. |
| `MEDULLA-REPO-INGEST-R652.2` | `crates/medulla-document/tests/processing.rs::tree_nests_and_covers_text_property` (planned) | L2 | It runs a property test over generated markdown. |
| `MEDULLA-REPO-INGEST-R652.3` | `tests/web_document_enrich_served.rs::section_tree_and_links_are_written` (planned) | L3 | Reads the `DocumentSection` nodes with `sectionOf`/`subsectionOf` edges, each chunk's `inSection` edge, the `ProcessedDocument` node (`tree_check` `valid`, `coverage_milli` 1000) and both `linksTo` edges; re-enriching changed text leaves no section or record of the old digest. |
| `MEDULLA-REPO-INGEST-R652.4` | `tests/web_document_enrich_served.rs::oversized_and_invalid_tree_refuse_without_writes` (planned) | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R652.5` | `tests/web_document_enrich_served.rs::processed_document_reads_back` (planned) | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R652.6` | `crates/medulla-document/benches/document_chunk_tree.rs::markdown` | L5 | Test: nightly bench `cargo bench -p medulla-document document_chunk_tree` (L5). |
| `MEDULLA-REPO-INGEST-R002.650` | `crates/medulla-types/tests/ingest_capabilities.rs::types_round_trip_and_contract_digest_changes_on_field_change` (planned) | L1 | Verified by the root slice test `tests/test_source_ingest_capabilities_served.py::test_envelope_commit_supersede_and_dead_letter_journey`. |
| `MEDULLA-REPO-INGEST-R002.651.1` | `tests/test_change_envelope_draft_e2e.py::test_replayed_envelope_writes_nothing` (planned) | L3 | It applies one envelope twice and compares the store digest. |
| `MEDULLA-REPO-INGEST-R002.651.2` | `tests/test_source_ingest_capabilities_served.py::test_supersession_edge_and_unknown_fact_refusal` (planned) | L3 | Reads the `supersedes` edge with `envelope_id` and the old fact's `superseded_by`; an absent old fact refuses `SUPERSESSION_UNKNOWN_FACT` and an edge without its CAS refuses `SUPERSESSION_UNPAIRED`, both with an unchanged store digest. Also `crates/medulla-h-graph/src/supersession_admission.rs::tests::unknown_and_unpaired_supersessions_are_refused` (L1). |
| `MEDULLA-REPO-INGEST-R002.651.3` | `tests/test_source_ingest_capabilities_served.py::test_dead_letter_list_and_redrive` (planned), `crates/medulla-jobs/src/store/task_queue.rs::tests::redrive_is_all_or_nothing` (planned) | L3, L1 | A dead-lettered task is listed, redriven to a claimable attempt 1 and removed; an unknown id refuses with nothing changed. |
| `MEDULLA-REPO-INGEST-R002.651.4` | `tests/test_process_conformance_served.py::test_conforming_trace_scores_one_deviating_below` (planned) | L3 | A conforming trace scores 1000 milli, a reversed step scores below 1000, an absent `process_model_id` refuses `PROCESS_MODEL_UNKNOWN`. Also `crates/medulla-mining/src/mining/process_mining.rs::tests::footprint_replay_counts_missing_and_remaining_tokens` (L1). |
| `MEDULLA-REPO-INGEST-R002.652` | `tests/test_source_ingest_capabilities_served.py::test_each_method_is_served_and_contract_listed` (planned) | L4 | It calls each method over the wire client. |
| `MEDULLA-REPO-INGEST-R002.653` | `tests/test_source_ingest_capabilities_served.py::test_refusals_leave_store_digest_unchanged` (planned) | L3 | It asserts each code and an equal store digest. |
| `MEDULLA-REPO-INGEST-R002.654` | `tests/test_source_ingest_capabilities_served.py::test_evidence_reads_back_under_second_principal` (planned) | L3 | The named test passes. |
| `MEDULLA-REPO-INGEST-R002.655` | `crates/medulla-server-graph/benches/change_envelope_commit.rs::change_envelope_commit` (planned) | L5 | Test: nightly bench group `ingest/change_envelope_commit` (L5). The served journey `tests/test_source_ingest_capabilities_served.py` (L3) gates the PR. |
| `MEDULLA-REPO-INGEST-R006.5` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_repository_index_tests.rs::one_parse_per_unique_blob_across_branches` (planned) | L3 | Test: a served repository-ingest test over branches that share history asserts one parse per unique blob (L3). |
| `MEDULLA-REPO-INGEST-R006.6` | `tests/ingest/test_paper_intake_schedule_served.py::test_firing_enqueues_one_task_per_boundary` (planned) | L3 | advances the engine clock across two boundaries. |
| `MEDULLA-REPO-INGEST-R603.7` | RETIRED: a second classification route; served by R603.3. | - | Retired; see `MEDULLA-REPO-INGEST-R603.3`. |
| `MEDULLA-REPO-INGEST-R700.1` | `crates/medulla-ingestion-types/src/code_health.rs::the_types_refuse_undecidable_dead_findings_and_the_golden_vector_round_trips` | L1 | Compile-time test: the types reject the invalid shapes named in `.4`; golden serialization vector round-trips. |
| `MEDULLA-REPO-INGEST-R700.2` | `crates/medulla-ingestion-types/src/code_health.rs::reachability_is_linear_and_bounded_on_10_1000_and_100000_symbols` | L2 | Unit tests cover the stated complexity bound on fixtures of 10, 1,000 and 100,000 items. |
| `MEDULLA-REPO-INGEST-R700.3` | `tests/repo_index/test_code_health_served.py::test_declared_entry_points_report_unreachable_symbols` (planned) | L3 | With `entry_points` `["main"]`, one `unreachable` finding for `b` lists `main` as tried; nothing is written. |
| `MEDULLA-REPO-INGEST-R700.4` | `crates/medulla-ingestion-types/src/code_health.rs::no_entry_points_reports_unknown_never_dead` | L1 | Negative tests assert each refusal code and that durable state is unchanged. |
| `MEDULLA-REPO-INGEST-R700.5` | `crates/medulla-ingestion-types/src/code_health.rs::each_finding_lists_its_entry_points_and_reproduces_from_its_inputs` | L1 | A read of the stored record reproduces the recorded decision from its recorded inputs. |
| `MEDULLA-REPO-INGEST-R700.6` | `tests/repo_index/test_code_health_served.py::test_no_entry_points_reports_unknown` (planned) | L5 | Test: the served test (L3) plus the nightly bench `repo_index/code_health` (L5). |
| `MEDULLA-REPO-INGEST-R801` | `Method::Uql` (planned) | L3 | A served test indexes a fixture workspace and reads the packages, layers and edges through `Method::Uql`. |

## Scenario matrix

| Test ID | Covers | Fixture and action | Expected result | Tier |
|---|---|---|---|---|
| RI-T01 | RI-01, RI-02 | Submit two immutable refs sharing one blob, a divergent same-path blob, and identical declarations in different files. Permute input order. | One parse per unique submitted blob, distinct occurrence IDs, exact ref/file memberships, stable canonical IDs and write-set digest. | Unit + integration |
| RI-T02 | RI-01, RI-07 | Submit absolute path, traversal path, malformed revision/digest, duplicate conflicting membership, wrong graph grant and cross-principal repository reference. | Each refuses before graph/CAS mutation with declared code; graph version and source marker unchanged. | Contract + integration |
| RI-T03 | RI-03 | Submit supported source with declarations, valid empty source, unsupported extension and parser-error bytes. Then upgrade parser capability and retry the latter two. | Ordered one-to-one `success`/`success`/`unsupported`/`error` outcomes, bounded diagnostics, new capability digest, no duplicate successful revision. | Unit + generated client |
| RI-T04 | RI-04 | Two modules with imported callable, receiver method, ambiguous same-name export and external package; repeat with randomized file ordering. | Exact resolved edges and counts for admitted targets; ambiguous/external remain unresolved; no dangling or nondeterministic edges. | Unit + integration |
| RI-T05 | RI-04, RI-10 | Similar functions and source/spec/test/release artifacts with one missing target. Change score ties and endpoint relation collision. | Versioned repeatable similarity and provenance. Missing target unresolved. Multiple legal relations retained per chosen edge model. | Integration |
| RI-T06 | RI-05 | Index, resend identical batch, resend same external key with changed content, kill/restart after CAS admission and after commit. | Replay does not advance graph version; conflict rejects; no partial graph or false receipt; committed snapshot survives restart. | Durable integration |
| RI-T07 | RI-06 | Rename, delete one path, delete a ref, then add the old blob in another ref. | Only named live memberships move/remove; other refs and immutable revision provenance remain queryable. | Durable integration |
| RI-T08 | RI-07 | Source contains harmless `@` and repository-relative `home/` text plus a host email, absolute host path and foreign tenant lookup. | Useful code remains; host identity redacted in materialized values; unauthorized read/write denied without content leak. | Security integration |
| RI-T09 | RI-08 | Generate a batch crossing operation, 48 MiB write-set and 64 MiB durable-record ceilings; split and resend. | Stable `REPOSITORY_BATCH_TOO_LARGE`, zero partial graph writes, split convergence to same final membership. | Boundary integration |
| RI-T10 | RI-09 | Native parse with deliberately unavailable model endpoint; enqueue funded, underfunded and unauthorized enrichment; restart while lease held. | Native commit completes; funded work drains once, underfunded parks visibly, unauthorized refuses, lease resumes without duplicate effects. | Durable integration |
| RI-T11 | RI-09 | Query an unenriched symbol with a small on-demand budget and a larger requested budget. | Small request gets bounded enrichment or explicit abstention; over-budget request refuses; original revision and native facts stay stable. | Query integration |
| RI-T12 | RI-11 | Full/delta/reconcile source snapshots with missing/stale expected checkpoint, empty approved and unapproved reconcile, mapping withdrawal, crash/restart and duplicate request. | Approved mappings only; exact cursor CAS; tombstones and receipt atomic; status after restart equals committed checkpoint; replay returns same receipt identity. | Durable contract |
| RI-T13 | RI-12 | Deliver ordered, duplicate, out-of-order and deleted source changes; query accelerated route before/after consumer watermark. | Idempotent output, explicit deletion, stale/out-of-order refusal, bounded freshness wait and route explanation. | Integration |
| RI-T14 | RI-01–RI-12 | Index a pinned public repository fixture with mixed supported/unsupported files. Record file/blob count, bytes, parser distribution, p50/p95 native latency, and exact engine revision. | Reproducible benchmark receipt and a separately approved threshold. No unsupported claim that indexing is “seconds” from fixture-only tests. | Performance/manual |

## Existing test entrypoints to extend

- `crates/medulla-compute/src/parser/branch_index_tests.rs` for branch scope, occurrence IDs and deterministic resolution.
- `tests/repository_index_durable.rs` for graph-scoped envelope, replay, tombstone, restart and CAS holder behavior.
- `tests/test_index_repository_result_contract.py` and generated-client tests for typed outcomes and refusal codes.
- `crates/medulla-source-types/src/source_ingestion/tests.rs` and server source-ingestion tests for checkpoint and mapping rules.
- `crates/medulla-server-graph/src/dispatch/graph_pipeline/repository_index/enrichment.rs` and repository worker tests for admission, lease and budget.

## Existing test entrypoints to extend

- `crates/medulla-compute/src/parser/branch_index_tests.rs` for branch scope, occurrence IDs and deterministic resolution.
- `tests/repository_index_durable.rs` for graph-scoped envelope, replay, tombstone, restart and CAS holder behavior.
- `tests/test_index_repository_result_contract.py` and generated-client tests for typed outcomes and refusal codes.
- `crates/medulla-source-types/src/source_ingestion/tests.rs` and server source-ingestion tests for checkpoint and mapping rules.
- `crates/medulla-server-graph/src/dispatch/graph_pipeline/repository_index/enrichment.rs` and repository worker tests for admission, lease and budget.

### Inventory authority and map generation proof

`tests/repository-index-and-ingestion/test_authority_binding_served.py::test_inventory_advance_fences_map_and_rebinds_symbols` is the planned served test. It exercises every schema and refusal rule in plan.md through generated served clients. Independently read admitted authority, producer receipts, exact revisions and output after restart. Test identical replay and conflicting source events, stale expected revisions, missing/changed authority, denied tenant and unknown references. Verify refusal stores nothing. Preserve every existing acceptance clause above verbatim.
Advance inventory before indexing completes. Both changed and unchanged symbol queries and old cursors must refuse. Reindex; read current span/revision for the changed symbol and stable ID/new generation/current revision for the unchanged symbol. Test rename, deletion, incomplete manifest and raced publication.

Test same-content renames with unchanged grammar suffix and parser/identity scheme: retain symbol IDs while updating file-version and membership. Change the grammar suffix and require reparse; allow changed symbol IDs. Also reparse after parser/identity scheme changes. Independently read current spans, revisions and generation in each case.

- **MEDULLA-REPO-INGEST-R002.1.6:** `crates/medulla-h-ingest/src/source_ingestion.rs::admit_kind_tests::skill_documents_is_admitted_and_facts_stays_refused` (planned, L1) and `crates/medulla-h-ingest/src/source_ingestion/skill_document.rs::tests::a_document_compiles_to_one_node_and_a_refusal_writes_nothing` (planned, L1).

## Explicit state machine rows (Article XXXI)

- `MEDULLA-REPO-INGEST-R802`: RETIRED: no index status field moves after construction.
- `MEDULLA-REPO-INGEST-R802.1`: RETIRED: `IndexFileStatus` is set once at construction; see `MEDULLA-REPO-INGEST-R006.4`.
- `MEDULLA-REPO-INGEST-R802.2`: RETIRED: `IndexRefStatus` is sender-declared and never reassigned; see `MEDULLA-REPO-INGEST-R006.5`.
