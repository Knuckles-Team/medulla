# MEDULLA-ENGINE-PARITY — Test specification

Governing spec: [spec.md](spec.md).

Proof levels follow the integration proof standard of `specs/decision-engine/test-spec.md`: L1 unit, L2 property and golden vector, L3 served Rust, L4 served client, L6 nightly. A test marked planned does not exist yet and no delivery is claimed. A nightly benchmark runs on the soak host and never gates a pull request.

## Row tests

| Row | Proving tests | State |
|---|---|---|
| `MEDULLA-ENGINE-PARITY-R001.1` | `crates/medulla-persistence/src/redb_backend/group_commit_queue_tests.rs::enqueue_timeout_parses_and_clamps`; `tests/group_commit_served.rs::full_queue_refuses_commit_queue_full` | planned |
| `MEDULLA-ENGINE-PARITY-R001.2` | `tests/group_commit_served.rs::full_queue_refuses_commit_queue_full`; `tests/group_commit_served.rs::one_failure_does_not_fail_batch` | planned |
| `MEDULLA-ENGINE-PARITY-R001.3` | `tests/group_commit_served.rs::sixty_four_writers_share_one_fsync`; bench group `engine_parity/group_commit` | planned |
| `MEDULLA-ENGINE-PARITY-R002.1` | `crates/medulla-transaction/tests/occ.rs::write_skew_is_detected` | existing |
| `MEDULLA-ENGINE-PARITY-R002.2` | `crates/medulla-transaction/tests/occ.rs::conflict_refuses_and_writes_nothing`; `crates/medulla-sql-tests/tests/user_tables.rs::stale_snapshot_commit_refuses`; `tests/pgwire_roundtrip.rs::occ_conflict_over_wire` | planned |
| `MEDULLA-ENGINE-PARITY-R002.3` | `tests/pgwire_roundtrip.rs::occ_conflict_over_wire`; bench group `engine_parity/occ_validate` | planned |
| `MEDULLA-ENGINE-PARITY-R004.1` | `crates/medulla-transaction/tests/occ.rs::disjoint_footprints_never_wait` | existing |
| `MEDULLA-ENGINE-PARITY-R004.2` | `crates/medulla-sql-tests/tests/user_tables.rs::retry_bound_refuses`; `tests/pgwire_roundtrip.rs::disjoint_sessions_commit_in_parallel` | planned |
| `MEDULLA-ENGINE-PARITY-R004.3` | `tests/pgwire_roundtrip.rs::disjoint_sessions_commit_in_parallel`; bench group `engine_parity/disjoint_writers` | planned |
| `MEDULLA-ENGINE-PARITY-R005.1` | `benches/engine_parity.rs` | existing |
| `MEDULLA-ENGINE-PARITY-R005.2` | `tests/test_engine_bench_gate.py::test_missing_workload_fails`; `tests/test_engine_bench_gate.py::test_regression_over_ten_percent_fails` | planned |
| `MEDULLA-ENGINE-PARITY-R005.3` | `tests/test_engine_bench_gate.py::test_baseline_file_matches_records_schema` | planned |
| `MEDULLA-ENGINE-PARITY-R006.1` | `crates/medulla-wire-core/src/wire/sqlstate.rs::tests::table_is_closed_and_unique`; `tests/pgwire_roundtrip.rs::extended_query_prepared_and_copy` | planned |
| `MEDULLA-ENGINE-PARITY-R006.2` | `tests/pgwire_roundtrip.rs::unsupported_message_refuses_0a000` | planned |
| `MEDULLA-ENGINE-PARITY-R006.3` | `tests/pgwire_roundtrip.rs::prepared_point_read_plan_cache_hits`; bench group `engine_parity/pgwire_prepared_read` | planned |
| `MEDULLA-ENGINE-PARITY-R007.1` | `crates/medulla-wire/src/pgwire/auth.rs::tests::three_login_kinds_resolve_one_principal`; `tests/pgwire_roundtrip.rs::wire_rls_equals_native_rls` | planned |
| `MEDULLA-ENGINE-PARITY-R007.2` | `tests/pgwire_roundtrip.rs::wire_rls_equals_native_rls`; `Method::Uql` | planned |
| `MEDULLA-ENGINE-PARITY-R007.3` | `tests/pgwire_roundtrip.rs::login_maps_to_principal`; bench group `engine_parity/pgwire_login` | planned |
| `MEDULLA-ENGINE-PARITY-R007.4` | `tests/identity/test_wire_login_served.py::test_grant_login_revoke` | planned |
| `MEDULLA-ENGINE-PARITY-R008.1` | `crates/medulla-sql-tests/tests/user_tables.rs::catalog_lists_constraints_and_indexes`; `tests/pgwire_roundtrip.rs::bi_tool_introspection_queries_pass` | planned |
| `MEDULLA-ENGINE-PARITY-R008.2` | `tests/pgwire_roundtrip.rs::bi_tool_introspection_queries_pass` | planned |
| `MEDULLA-ENGINE-PARITY-R008.3` | `tests/pgwire_roundtrip.rs::introspection_fixture_total_latency`; bench group `engine_parity/introspection` | planned |
| `MEDULLA-ENGINE-PARITY-R009.1` | `crates/medulla-sql-tests/tests/compat.rs::matrix_loads_and_is_closed`; `tests/test_sql_compat_conformance.py::test_supported_entries_pass` | planned |
| `MEDULLA-ENGINE-PARITY-R009.2` | `crates/medulla-sql-tests/tests/compat.rs::every_supported_entry_has_a_test`; `tests/pgwire_roundtrip.rs::refused_form_returns_0a000` | planned |
| `MEDULLA-ENGINE-PARITY-R009.3` | `tests/test_sql_compat_conformance.py::test_supported_entries_pass` | planned |
| `MEDULLA-ENGINE-PARITY-R010.1` | `crates/medulla-sql-tests/tests/index.rs::partial_index_holds_only_matching_rows`; `tests/pgwire_roundtrip.rs::create_partial_index_over_wire_is_used` | planned |
| `MEDULLA-ENGINE-PARITY-R010.2` | `crates/medulla-sql-tests/tests/index.rs::planner_uses_partial_index`; `crates/medulla-sql-tests/tests/index.rs::nondeterministic_expression_refuses` | planned |
| `MEDULLA-ENGINE-PARITY-R010.3` | `tests/pgwire_roundtrip.rs::create_partial_index_over_wire_is_used`; bench group `engine_parity/index_probe` | planned |
| `MEDULLA-ENGINE-PARITY-R011.1` | `crates/medulla-sql-tests/tests/constraint.rs::deferred_foreign_key_checks_at_commit`; `tests/pgwire_roundtrip.rs::constraint_violations_return_sqlstate` | planned |
| `MEDULLA-ENGINE-PARITY-R011.2` | `tests/pgwire_roundtrip.rs::constraint_violations_return_sqlstate` | planned |
| `MEDULLA-ENGINE-PARITY-R011.3` | `tests/pgwire_roundtrip.rs::constraints_hold_under_concurrent_sessions`; bench group `engine_parity/constraints` | planned |
| `MEDULLA-ENGINE-PARITY-R012.1` | `crates/medulla-sql-tests/tests/trigger.rs::before_insert_rewrites_row`; `tests/pgwire_roundtrip.rs::before_insert_trigger_rewrites_row` | planned |
| `MEDULLA-ENGINE-PARITY-R012.2` | `crates/medulla-sql-tests/tests/trigger.rs::depth_bound_refuses` | planned |
| `MEDULLA-ENGINE-PARITY-R012.3` | `tests/pgwire_roundtrip.rs::after_insert_trigger_runs_once_per_row`; bench group `engine_parity/trigger_after_insert` | planned |
| `MEDULLA-ENGINE-PARITY-R013.1` | `crates/medulla-sql-tests/tests/sequence.rs::values_never_repeat_after_crash`; `tests/pgwire_roundtrip.rs::serial_column_uses_sequence` | planned |
| `MEDULLA-ENGINE-PARITY-R013.2` | `crates/medulla-sql-tests/tests/sequence.rs::setval_below_high_mark_refuses` | planned |
| `MEDULLA-ENGINE-PARITY-R013.3` | `tests/pgwire_roundtrip.rs::serial_column_uses_sequence`; bench group `engine_parity/sequence_nextval` | planned |
| `MEDULLA-ENGINE-PARITY-R014.1` | `crates/medulla-sql-tests/tests/matview.rs::refresh_recomputes_and_records_input_versions`; `tests/pgwire_roundtrip.rs::view_and_matview_over_wire` | planned |
| `MEDULLA-ENGINE-PARITY-R014.2` | `tests/pgwire_roundtrip.rs::view_and_matview_over_wire` | planned |
| `MEDULLA-ENGINE-PARITY-R014.3` | `tests/pgwire_roundtrip.rs::matview_refresh_checksum`; bench group `engine_parity/matview_refresh` | planned |
| `MEDULLA-ENGINE-PARITY-R015.1` | `crates/medulla-server-cdc/src/cdc.rs::tests::row_changes_emit_in_commit_order`; `tests/test_change_subscribe_served.py::test_cdc_read_resumes_and_expires` | planned |
| `MEDULLA-ENGINE-PARITY-R015.2` | `tests/test_change_subscribe_served.py::test_cdc_read_resumes_and_expires` | planned |
| `MEDULLA-ENGINE-PARITY-R015.3` | `tests/test_change_subscribe_served.py::test_thousand_changes_arrive_in_order`; bench group `engine_parity/cdc_emit` | planned |
| `MEDULLA-ENGINE-PARITY-R016.1` | `crates/medulla-storage/tests/archive.rs::restore_reaches_target_seq`; `tests/test_pitr_served.py::test_restore_to_time` | planned |
| `MEDULLA-ENGINE-PARITY-R016.2` | `tests/test_pitr_served.py::test_restore_to_time`; `tests/test_pitr_served.py::test_target_past_archive_refuses` | planned |
| `MEDULLA-ENGINE-PARITY-R016.3` | `tests/test_pitr_served.py::test_restore_drill_no_lost_sequences`; bench group `engine_parity/pitr_restore_drill` | planned |
| `MEDULLA-ENGINE-PARITY-R019.1` | `crates/medulla-storage/tests/provider.rs::provider_set_is_closed`; `tests/test_external_source_boundary.py::test_engine_data_never_syncs_out` | planned |
| `MEDULLA-ENGINE-PARITY-R019.2` | `tests/test_external_source_boundary.py::test_engine_data_never_syncs_out` | planned |
| `MEDULLA-ENGINE-PARITY-R019.3` | `tests/test_external_source_boundary.py::test_federated_join_pushes_down`; bench group `engine_parity/external_join` | planned |
| `MEDULLA-ENGINE-PARITY-R020.1` | `crates/medulla-types/tests/engine_capabilities.rs::report_round_trips` | existing |
| `MEDULLA-ENGINE-PARITY-R020.2` | `tests/test_engine_capabilities_served.py::test_report_matches_features` | planned |

## Negative and boundary cases

- A conflicting transaction refuses with `TXN_CONFLICT` and writes nothing (`MEDULLA-ENGINE-PARITY-R002.2`).
- A full commit queue refuses with `COMMIT_QUEUE_FULL` and changes no state (`MEDULLA-ENGINE-PARITY-R001.2`).
- An unsupported wire message or a refused statement form returns SQLSTATE `0A000` (`MEDULLA-ENGINE-PARITY-R006.2`, `MEDULLA-ENGINE-PARITY-R009.2`).
- An unknown, revoked or mismatched wire login refuses with SQLSTATE `28000` (`MEDULLA-ENGINE-PARITY-R007.1`).
- A constraint violation returns its SQLSTATE and the constraint name (`MEDULLA-ENGINE-PARITY-R011.2`).
- A trigger chain deeper than 16 refuses with `TRIGGER_DEPTH` (`MEDULLA-ENGINE-PARITY-R012.2`).
- A sequence value below the high mark refuses with `SEQUENCE_REWIND` (`MEDULLA-ENGINE-PARITY-R013.2`).
- A change cursor below the retained window refuses with `CURSOR_EXPIRED` (`MEDULLA-ENGINE-PARITY-R015.2`).
- A recovery target past the archived log refuses with `RECOVERY_TARGET_UNAVAILABLE` (`MEDULLA-ENGINE-PARITY-R016.2`).
- A request to place engine data in an outside database refuses with `EXTERNAL_PLACEMENT_FORBIDDEN` (`MEDULLA-ENGINE-PARITY-R019.2`).

## Integration

- A standard PostgreSQL client connects over TLS, introspects the catalog, runs prepared statements and COPY, and reads under row security (`tests/pgwire_roundtrip.rs`).
- 64 concurrent writers share one fsync; disjoint writers never wait (`MEDULLA-ENGINE-PARITY-R001.3`, `MEDULLA-ENGINE-PARITY-R004.3`).
- A subscriber follows the change feed, stops and resumes from its cursor (`MEDULLA-ENGINE-PARITY-R015.2`).
- A restore to a chosen sequence matches the fingerprint recorded at that sequence (`MEDULLA-ENGINE-PARITY-R016.1`).

## Quality and release proof

- Clippy, tests, jscpd, CCCC, dupehound and KISS pass on the changed crates.
- The no-duplicate-authority check confirms one committer per store file and one change feed.
- Each nightly benchmark gates a 10 percent regression against its stored baseline (`MEDULLA-ENGINE-PARITY-R005.2`).
