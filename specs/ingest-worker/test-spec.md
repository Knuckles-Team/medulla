# MEDULLA-INGEST-WORKER — Test specification

Each leaf row names its proof. A test marked `(planned)` in the row does not exist yet and lands with the row. Served proofs start a disposable local Medulla instance and drive the Python client; they create their own tenant and graph fixtures.

## Proof levels

| Level | Harness | Verifies a row alone |
|---|---|---|
| L1 Unit | `#[cfg(test)]` module or crate test | No |
| L2 Property and scale | scaling fixtures of 10, 1,000 and 100,000 items | No |
| L3 Handler | Rust handler test through the real function with a recording resolver | No |
| L4 Served | Python client through the production dispatch with a tenant and principal | Yes |
| L5 Nightly bench | `cargo bench` on the reference host; a service level, never a pull request gate | No |

## Row proofs

| Row | Proof | Level | State |
|---|---|---|---|
| `MEDULLA-INGEST-WORKER-R001.1` | `crates/medulla-source-types/src/ingest_job.rs::tests::envelope_round_trips_and_refuses_invalid_shapes` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R001.2` | `crates/medulla-source-types/src/ingest_job.rs::tests::lane_for_every_kind` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R001.3` | `crates/medulla-source-types/src/ingest_job.rs::tests::partition_key_orders_per_tenant_and_corpus` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R001.4` | `tests/ingest/test_ingest_queue_served.py::test_duplicate_enqueue_is_one_task` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.7` | `crates/medulla-source-types/src/ingest_job.rs::tests::retry_policy_refusals` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R001.8` | `crates/medulla-source-types/src/ingest_job.rs::tests::next_delay_is_bounded_growing_and_seeded` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R001.9` | `tests/ingest/test_ingest_queue_served.py::test_failed_job_retries_then_dead_letters` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.10` | `tests/ingest/test_ingest_queue_served.py::test_hydration_kinds_use_hydration_queue` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.11` | `tests/ingest/test_ingest_queue_served.py::test_full_queue_refuses_and_depth_is_unchanged` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.12` | `tests/ingest/test_ingest_queue_served.py::test_lapsed_lease_redelivers_then_exhausts` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.13` | `crates/medulla-kernel/src/metrics/tests.rs::ingest_metrics_after_success_retry_dead_letter` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R001.14` | `tests/ingest/test_ingest_worker_contract_served.py::test_runner_call_sequence` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.1006` | `crates/medulla-types/src/data_prep_declaration.rs::tests::declaration_round_trips_and_rejects_unknown_fields` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R001.1007` | `crates/medulla-h-ingest/src/data_prep_declaration.rs::tests::replace_is_atomic_and_equal_digest_is_unchanged` | L2 | exists |
| `MEDULLA-INGEST-WORKER-R001.1008` | `tests/ingest/test_data_prep_declaration_served.py::test_declare_then_read_through_the_python_client` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R001.1009` | `tests/ingest/test_data_prep_declaration_served.py::test_declare_refusals_change_nothing` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R001.1010` | `tests/ingest/test_data_prep_declaration_served.py::test_report_counts_written_unchanged_refused` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R001.1011` | `benches/data_prep_declare.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R002.1` | `crates/medulla-types/src/ontology_proposal.rs::tests::proposal_round_trips_and_starts_as_draft` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R002.2` | `crates/medulla-types/src/ontology_proposal.rs::tests::score_proposal_checks_coverage_and_redundancy` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R002.3` | `crates/medulla-h-ingest/src/source_ingestion/proposals.rs::tests::judge_runs_inside_prepare` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R002.4` | `crates/medulla-types/tests/ingest_rules.rs::a_failing_proposal_is_refused_stays_draft_and_allows_no_instances` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R002.5` | `tests/ingest/test_ontology_induction_served.py::test_receipt_reports_scores_and_replays` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R002.6` | `tests/ingest/test_ontology_induction_served.py::test_passing_proposal_admitted_failing_stays_draft` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R002.7` | `tests/ingest/test_ontology_induction_served.py::test_draft_proposal_takes_no_instances` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.1` | `crates/medulla-types/src/knowledge_atom.rs::tests::atom_round_trips_and_rejects_unknown_kind` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R003.2` | `crates/medulla-document/src/atoms.rs::tests::decompose_kinds_and_split` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R003.3` | `tests/ingest/test_knowledge_atoms_served.py::test_ingest_then_context_cites_provenance` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.4` | `crates/medulla-types/tests/ingest_rules.rs::an_atom_without_provenance_is_refused_and_no_relevant_atom_gives_an_empty_context` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R003.5` | `tests/ingest/test_knowledge_atoms_served.py::test_counts_in_receipt_and_context` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.6` | `tests/ingest/test_knowledge_atoms_served.py::test_ingest_then_context_cites_provenance` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.7` | `crates/medulla-source-types/src/source_document.rs::tests::document_receipt_round_trips` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R003.8` | `crates/medulla-h-ingest/src/source_ingestion/documents.rs::tests::complete_document_is_idempotent` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.9` | `crates/medulla-h-ingest/src/source_ingestion/documents.rs::tests::prepare_validates_document_after_resolution` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.10.1` | `crates/medulla-h-ingest/src/source_ingestion/documents.rs::tests::invalid_document_quarantines_without_partial_graph` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.11` | `crates/medulla-h-ingest/src/source_ingestion/documents.rs::tests::quarantine_finish_and_replay_preserve_outcome` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.12` | `tests/ingest/test_document_normalization_served.py::test_document_batch_is_atomic` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.13` | `crates/medulla-source-types/src/source_transcript.rs::tests::transcript_round_trips` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R003.14` | `crates/medulla-h-ingest/src/source_ingestion/transcript.rs::tests::source_sequence_and_identity_are_preserved` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R003.15` | `crates/medulla-h-ingest/src/source_ingestion/transcript.rs::tests::prepare_routes_declared_transcript_format` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.16` | `crates/medulla-h-ingest/src/source_ingestion/transcript.rs::tests::unknown_format_and_foreign_concept_are_refused` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R003.17.1` | `tests/ingest/test_transcript_served.py::test_same_tenant_unauthorized_reads_are_denied` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R003.18` | `tests/ingest/test_transcript_served.py::test_thread_messages_are_shared_within_scope` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R604.1` | `crates/medulla-types/src/ingest_lanes.rs::tests::lane_ids_validate_and_round_trip` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R604.2` | `crates/medulla-types/src/ingest_lanes.rs::tests::every_stage_has_one_class` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R604.3` | `tests/ingest/test_lane_admission_served.py::test_claim_enforces_admission` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R604.4` | `tests/ingest/test_lane_admission_served.py::test_reserve_refusal_has_code_and_retry_hint` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R604.5` | `crates/medulla-kernel/src/metrics/tests.rs::lane_gauges_after_success_and_refusal` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R604.6` | `benches/ingest_claim_admit.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R605.1` | `crates/medulla-types/src/ingest_drain.rs::tests::types_round_trip_and_reject_bad_ids` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R605.2` | `crates/medulla-h-ingest/src/ingest_drain.rs::tests::ack_with_cursor_enqueues_next_page` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R605.3` | `tests/ingest/test_source_drain_served.py::test_ingest_lands_in_the_routed_graph` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R605.4` | `tests/ingest/test_source_drain_served.py::test_redelivered_page_is_dropped` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R605.5` | `tests/ingest/test_source_drain_served.py::test_drain_chains_pages_and_reports` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R605.6` | `benches/ingest_drain.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R606.1` | `crates/medulla-types/src/ingest_materialize.rs::tests::report_round_trips_and_sums` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R606.2` | `crates/medulla-h-ingest/src/source_ingestion.rs::tests::stamps_and_label_normalization` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R606.3` | `tests/ingest/test_materialize_served.py::test_canned_vendor_batch_is_admitted` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R606.4` | `tests/ingest/test_materialize_served.py::test_batch_without_source_system_is_refused_whole` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R606.5` | `tests/ingest/test_materialize_served.py::test_receipt_reports_per_class` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R606.6` | `benches/ingest_materialize.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R700.1` | `crates/medulla-document/src/fragment.rs::tests::golden_fragment_round_trips_and_rejects_bad_spans` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R700.2` | `crates/medulla-document/tests/readers_scale.rs::time_and_memory_scale_linearly` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R700.3` | `tests/ingest/test_readers_served.py::test_each_format_yields_spanned_fragments` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R700.4` | `tests/ingest/test_readers_served.py::test_reader_refusals_quarantine_without_projection` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R700.5` | `crates/medulla-document/tests/readers_scale.rs::spans_reextract_to_fragment_text` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R700.6` | `tests/ingest/test_readers_served.py::test_each_format_yields_spanned_fragments` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R701.1` | `crates/medulla-types/src/extracted_fact.rs::tests::golden_vector_round_trips_and_invalid_shapes_are_rejected` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R701.2` | `crates/medulla-types/tests/ingest_rules.rs::grounding_repairs_direction_and_deduper_scales_linearly` | L2 | exists |
| `MEDULLA-INGEST-WORKER-R701.3` | `crates/medulla-h-ingest/src/source_ingestion/facts.rs::tests::facts_reach_the_atom_store` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R701.4` | `crates/medulla-types/tests/ingest_rules.rs::a_fact_without_evidence_is_refused_and_an_unfit_fact_stays_an_ungrounded_claim` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R701.5` | `tests/ingest/test_fact_grounding_served.py::test_unchanged_content_is_unchanged` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R701.6` | `tests/ingest/test_fact_grounding_served.py::test_direction_repair_and_ungrounded_claims` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R701.7` | `tests/ingest/test_fact_grounding_served.py::test_link_needs_shared_mentions` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R702.1` | `crates/medulla-compute/src/sketch.rs::tests::lsh_config_validates_and_round_trips` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R702.2` | `crates/medulla-compute/src/sketch.rs::tests::lsh_scaling_and_fn_rate` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R702.3` | `crates/medulla-h-ingest/src/source_ingestion/documents.rs::tests::dedup_runs_before_atom_storage` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R702.4` | `tests/ingest/test_lsh_dedup_served.py::test_near_duplicates_link_and_distinct_pass` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R702.5` | `tests/ingest/test_lsh_dedup_served.py::test_link_stores_similarity_estimate` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R702.6` | `tests/ingest/test_lsh_dedup_served.py::test_near_duplicates_link_and_distinct_pass` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R900.1` | `crates/medulla-types/src/source_governance.rs::tests::record_governance_round_trips_and_rejects_invalid_shapes` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R900.2` | `crates/medulla-h-ingest/src/source_ingestion/governance.rs::tests::public_restricted_marked_and_hold_records` | L2 | exists |
| `MEDULLA-INGEST-WORKER-R900.3` | `tests/ingest/test_source_governance_served.py::test_policy_and_access_exist_with_the_node` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R900.4` | `crates/medulla-types/tests/ingest_rules.rs::conflicting_or_retention_less_governance_is_refused` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R900.5` | `tests/ingest/test_source_governance_served.py::test_receipt_counts_governed_refused_marked` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R900.6` | `benches/ingest_source_governance.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R901.1` | `crates/medulla-types/src/source_governance.rs::tests::source_row_version_round_trips_and_rejects_empty_table` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R901.2` | `crates/medulla-h-ingest/src/source_ingestion/row_version.rs::tests::first_newer_and_replayed_versions` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R901.3` | `tests/ingest/test_row_version_served.py::test_versioned_record_reads_back_its_locus` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R901.4` | `crates/medulla-types/tests/ingest_rules.rs::non_integer_or_decreasing_row_versions_are_refused` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R901.5` | `tests/ingest/test_row_version_served.py::test_receipt_counts_stored_superseded_refused` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R901.6` | `benches/ingest_row_version.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R902.1` | `crates/medulla-types/tests/source_governance.rs::access_snapshot_round_trips` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R902.2` | `crates/medulla-h-ingest/src/source_ingestion/permissions.rs::tests::access_batch_leaves_content_unchanged` | L2 | exists |
| `MEDULLA-INGEST-WORKER-R902.3.1` | `crates/medulla-source-types/src/source_ingestion/tests.rs::batch_kind_requires_matching_extension` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R902.4` | `crates/medulla-types/tests/ingest_rules.rs::unknown_records_and_oversized_groups_are_refused` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R902.5` | `tests/ingest/test_source_permission_sync_served.py::test_stale_restricted_record_fails_closed` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R902.6` | `benches/ingest_permission_sync.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R903.1` | `crates/medulla-types/tests/source_governance.rs::reconcile_plan_round_trips` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R903.2` | `crates/medulla-h-ingest/src/source_ingestion/sweep.rs::tests::missing_records_delete_after_grace` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R903.3` | `tests/ingest/test_source_reconcile_served.py::test_enumeration_prunes_deleted_record` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R903.4` | `crates/medulla-types/tests/ingest_rules.rs::incomplete_enumerations_never_delete_and_mass_deletes_are_guarded` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R903.5` | `tests/ingest/test_source_reconcile_served.py::test_reconcile_receipt_reads_back` | L4 | planned |
| `MEDULLA-INGEST-WORKER-R903.6` | `benches/ingest_source_reconcile.rs` | L5 | exists |
| `MEDULLA-INGEST-WORKER-R904.1` | `crates/medulla-types/tests/source_governance.rs::container_round_trips` | L4 | exists |
| `MEDULLA-INGEST-WORKER-R904.2` | `crates/medulla-core/tests/source_hierarchy.rs::subtree_returns_descendants` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R904.3` | `tests/ingest/test_source_hierarchy_served.py::test_containers_ingest_and_scope` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R904.4` | `crates/medulla-types/tests/ingest_rules.rs::container_cycles_and_excess_depth_are_refused` | L1 | exists |
| `MEDULLA-INGEST-WORKER-R904.5` | `tests/ingest/test_source_hierarchy_served.py::test_container_tree_reads_back` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R904.6` | `tests/ingest/test_source_hierarchy_served.py::test_containers_ingest_and_scope` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R910.1` | `crates/medulla-h-ingest/src/source_ingestion.rs::tests::stream_object_id_ignores_batch_and_source_name` | L1 | planned |
| `MEDULLA-INGEST-WORKER-R910.2` | `crates/medulla-h-ingest/src/source_ingestion.rs::tests::previous_digest_is_none_then_prior_batch_digest` | L2 | planned |
| `MEDULLA-INGEST-WORKER-R910.3` | `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs::a_second_batch_to_the_same_stream_chains_its_content_version` | L3 | exists |
| `MEDULLA-INGEST-WORKER-R910.4` | `tests/ingest/test_source_stream_versions_served.py::test_racing_writers_one_stale_refusal` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R910.5` | `tests/ingest/test_source_stream_versions_served.py::test_receipt_names_digest_chain` | L3 | planned |
| `MEDULLA-INGEST-WORKER-R910.6` | `tests/ingest/test_source_stream_versions_served.py::test_second_batch_on_one_stream_admits` | L3 | planned |

## Negative and boundary cases

Blank job id or target, unknown job kind, a forged tenant, a duplicate delivery after ack, a full queue, a lapsed lease, a stale page cursor, a batch kind that does not match its extension, a classification that disagrees with the public flag, a decreasing row version, an incomplete enumeration, a container cycle, and an oversize, encrypted or corrupt document each leave durable state unchanged.

## Quality and release proof

Run `cargo test` for the touched crates, the served Python tests under `tests/ingest/`, and the spec gates `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
