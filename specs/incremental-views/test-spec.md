# MEDULLA-INCREMENTAL-VIEWS — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Served (L3) | Define `Scan Doc` then `Filter year > 2000`; 200 adds, 50 updates, 50 removes through `dispatch_on_heap`. | Each `PlanMatViewGet` equals `matview::materialize`; mode stays incremental. |  |
| T-002 | FR-002 | Unit (L2) | Define a plan with `Traverse`. | Recompute mode; reason code `MATVIEW_NOT_INCREMENTAL` and the op index. |  |
| T-003 | FR-003 | Unit (L1) | Deltas with repeated rows, zero sums and `i64::MAX` weights. | Weights add; zero rows drop; overflow refuses `DELTA_WEIGHT_OVERFLOW`. |  |
| T-004 | FR-004 | Property (L2) | Every accepted construct and its oracle case. | A construct without a case fails the coverage test by name. |  |
| T-005 | FR-005 | pgwire (L3) | One supported and one unsupported plan view. | `medulla_catalog.matviews` rows show mode and code; metrics render. |  |
| T-006 | FR-006 | Served (L3) | Define a view, restart the state, reload. | The reloaded view is incremental and equals a recompute. |  |
| T-007 | FR-007 | Served (L3) | Memory budget 16 MiB, ceiling 1 MiB, three views. | One view spills, one demotes with `STATE_BUDGET_EXCEEDED`, answers stay equal. |  |
| T-008 | FR-008 | Served (L3) | Check every 10 changes, tick 100 ms, corrupt one member. | Demotion with `SHADOW_MISMATCH` within 2 s; reads stay correct. |  |
| T-009 | FR-009 | pgwire (L3) | `orders` with 100,000 rows and the grouped view; 1,000 commits. | Each read equals the defining query; `stale` stays false. |  |
| T-010 | FR-010 | Served (L3) | `CdcRead` on `view:<name>` from seq 0; 100 changes. | Folded feed equals the view after each change; a gap resumes from a snapshot. |  |
| T-011 | FR-011 | Served (L3) | 10,000-class chain; remove one middle edge. | Closure equals a fresh classification; no full rebuild branch. |  |
| T-012 | FR-012 | pgwire (L3) | min, max and DISTINCT views; delete the current minimum. | Each read equals the defining query. |  |
| T-013 | FR-013 | pgwire (L3) | Indexed `orders` join `customers`; 500 commits touching both. | Each read equals the defining query. |  |
| T-014 | FR-014 | Docs (L1) | Parse `docs/interfaces/incremental-views.md`. | Every knob, code and metric matches the source. |  |

## Row bindings

Each row names its proving test. A shipped row is bound by a `// spec: <ID>` line above its test. A planned row gets the line when its test lands.

| Requirement | Name | Proving test |
|---|---|---|
| `MEDULLA-INCREMENTAL-VIEWS-R001` | Plan views are maintained from change deltas (rollup) | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::plan_view_served_define_write_get_equals_recompute` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.1` | Signed row deltas | `crates/medulla-exec/src/incremental/circuit.rs::tests::scan_add_update_remove_matches_recompute` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.2` | Compile or name the first unsupported op | `crates/medulla-exec/src/incremental/circuit.rs::tests::compile_rejects_unsupported_ops_with_index` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.3` | Membership views equal the executor after every change | `crates/medulla-plan/tests/incremental_execute_oracle.rs::membership_matches_execute` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.4` | Tumbling window aggregates equal the executor | `crates/medulla-plan/tests/incremental_execute_oracle.rs::window_matches_execute` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.5` | Maintenance cost follows the delta, not the view | `crates/medulla-plan/tests/incremental_o_delta.rs::maintenance_cost_is_o_delta_not_o_view` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.6` | Define, change and read are wired | `crates/medulla-server-cdc/src/matview/manager.rs::tests::apply_delta_maintains_incremental_view_end_to_end` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.7` | A fallback keeps a typed reason | `crates/medulla-server-cdc/src/matview/manager.rs::tests::note_fallback_records_queryable_reason` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.8` | EXPLAIN reports maintainability | `crates/medulla-plan/src/uql/tests/serve.rs::explain_reports_stages_estimates_and_incrementality` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.9` | Continuous aggregate queries fold the change feed | `crates/medulla-server-served-tests/src/tests.rs::test_continuous_query_incremental_equals_full_rerun` |
| `MEDULLA-INCREMENTAL-VIEWS-R001.10` | Served hot-path proof for plan views | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::plan_view_served_define_write_get_equals_recompute` |
| `MEDULLA-INCREMENTAL-VIEWS-R002` | One weighted delta type for every view family (rollup) | `crates/medulla-types/src/delta.rs::tests::consolidation_sums_weights_and_drops_zero` |
| `MEDULLA-INCREMENTAL-VIEWS-R002.1` | Move and widen the delta type | `crates/medulla-types/src/delta.rs::tests::insert_and_retract_carry_unit_weights` |
| `MEDULLA-INCREMENTAL-VIEWS-R002.2` | Consolidate and refuse overflow | `crates/medulla-types/src/delta.rs::tests::consolidation_sums_weights_and_drops_zero`, `crates/medulla-types/src/delta.rs::tests::weight_overflow_is_refused` |
| `MEDULLA-INCREMENTAL-VIEWS-R002.3` | Wiring: the circuit consolidates before it folds | `crates/medulla-server-cdc/src/matview/manager.rs::tests::apply_failure_demotes_with_typed_reason` |
| `MEDULLA-INCREMENTAL-VIEWS-R002.4` | Proof: shipped graph behavior is unchanged | `crates/medulla-plan/tests/incremental_execute_oracle.rs::membership_matches_execute` |
| `MEDULLA-INCREMENTAL-VIEWS-R003` | One differential oracle harness | `crates/medulla-exec/src/incremental/oracle.rs::tests::mismatch_names_step_and_row` |
| `MEDULLA-INCREMENTAL-VIEWS-R004` | No operator ships without an oracle case | `crates/medulla-plan/tests/incremental_oracle_coverage.rs::every_accepted_construct_has_an_oracle_case` |
| `MEDULLA-INCREMENTAL-VIEWS-R005` | Maintenance state is observable (rollup) | `tests/pgwire_roundtrip.rs::matview_status_catalog_reports_mode_and_reason` |
| `MEDULLA-INCREMENTAL-VIEWS-R005.1` | Model: the view status board | `crates/medulla-core/src/view_status.rs::tests::publish_replaces_and_remove_deletes` |
| `MEDULLA-INCREMENTAL-VIEWS-R005.2` | The manager publishes every state change | `crates/medulla-server-cdc/src/matview/manager.rs::tests::status_board_tracks_mode_and_changes` |
| `MEDULLA-INCREMENTAL-VIEWS-R005.3` | System table `medulla_catalog.matviews` | `crates/medulla-sql/src/sql/catalog_matviews.rs::tests::matviews_batch_has_fixed_columns` |
| `MEDULLA-INCREMENTAL-VIEWS-R005.4` | Prometheus view metrics | `crates/medulla-kernel/src/metrics/tests.rs::view_metrics_render_with_bounded_labels` |
| `MEDULLA-INCREMENTAL-VIEWS-R005.5` | Served proof of the status table | `tests/pgwire_roundtrip.rs::matview_status_catalog_reports_mode_and_reason` |
| `MEDULLA-INCREMENTAL-VIEWS-R006` | Boot restores incremental mode by reseeding | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::restart_reloads_plan_view_in_incremental_mode` |
| `MEDULLA-INCREMENTAL-VIEWS-R007` | Operator state is budgeted and spilled (rollup) | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::state_budget_spills_then_demotes_and_answers_stay_correct` |
| `MEDULLA-INCREMENTAL-VIEWS-R007.1` | State accounting | `crates/medulla-exec/src/incremental/circuit.rs::tests::state_bytes_tracks_inserts_and_retracts` |
| `MEDULLA-INCREMENTAL-VIEWS-R007.2` | Per-view ceiling demotes | `crates/medulla-server-cdc/src/matview/manager.rs::tests::view_over_ceiling_demotes_with_typed_reason` |
| `MEDULLA-INCREMENTAL-VIEWS-R007.3` | Spill and reload by least recent change | `crates/medulla-server-cdc/src/matview/manager.rs::tests::least_recent_view_spills_and_reloads_equal` |
| `MEDULLA-INCREMENTAL-VIEWS-R007.4` | Wiring: the store backs the spill | `crates/medulla-persistence/src/redb_backend/matview_api.rs::tests::operator_state_get_reads_one_row` |
| `MEDULLA-INCREMENTAL-VIEWS-R007.5` | Served proof of budget and spill | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::state_budget_spills_then_demotes_and_answers_stay_correct` |
| `MEDULLA-INCREMENTAL-VIEWS-R008` | Sampled shadow verification in production (rollup) | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::corrupted_state_is_caught_and_demoted` |
| `MEDULLA-INCREMENTAL-VIEWS-R008.1` | Model: the check schedule | `crates/medulla-server-cdc/src/matview/manager.rs::tests::check_becomes_due_after_knob_changes` |
| `MEDULLA-INCREMENTAL-VIEWS-R008.2` | Compare and demote | `crates/medulla-h-txn/src/matview_shadow.rs::tests::mismatch_demotes_and_match_keeps_mode` |
| `MEDULLA-INCREMENTAL-VIEWS-R008.3` | Wiring: the check task | `crates/medulla-h-txn/src/matview_shadow.rs::tests::tick_runs_due_checks_with_deadline` |
| `MEDULLA-INCREMENTAL-VIEWS-R008.4` | Served proof of shadow verification | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::corrupted_state_is_caught_and_demoted` |
| `MEDULLA-INCREMENTAL-VIEWS-R009` | SQL views over one table are maintained in the commit (rollup) | `tests/pgwire_roundtrip.rs::matview_incremental_read_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.1` | Model: the incremental view program | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::row_expr_three_valued_logic` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.2` | Refusal: name the first unsupported construct | `crates/medulla-sql-lang/src/sql/matview_compile.rs::tests::unsupported_constructs_name_the_first` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.3` | Behavior: linear maintenance in the commit | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::rows_view_tracks_insert_update_delete` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.4` | Behavior: grouped exact aggregates | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::groups_count_sum_avg_and_empty_group_leaves` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.5` | Wiring: create, skip the stale flag, publish status | `tests/pgwire_roundtrip.rs::matview_incremental_read_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.6` | SQL differential oracle | `crates/medulla-table-store/tests/matview_oracle.rs::supported_shapes_agree_after_every_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.7` | Served hot-path proof for SQL views | `tests/pgwire_roundtrip.rs::matview_incremental_read_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R009.8` | Nightly budget for SQL views | `crates/medulla-table-store/benches/matview_incremental.rs::groups` |
| `MEDULLA-INCREMENTAL-VIEWS-R010` | View changes ride the one change feed (rollup) | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::view_delta_feed_matches_result_difference` |
| `MEDULLA-INCREMENTAL-VIEWS-R010.1` | Plan views publish their output delta | `crates/medulla-server-cdc/src/cdc.rs::tests::view_output_rows_reach_their_feed` |
| `MEDULLA-INCREMENTAL-VIEWS-R010.2` | SQL views publish their output delta | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::maintain_views_reports_view_row_changes` |
| `MEDULLA-INCREMENTAL-VIEWS-R010.3` | Resume, expiry and snapshot | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::view_feed_gap_then_snapshot_resume` |
| `MEDULLA-INCREMENTAL-VIEWS-R010.4` | Served proof and arbora seam | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::view_delta_feed_matches_result_difference` |
| `MEDULLA-INCREMENTAL-VIEWS-R011` | Ontology closure updates from fact deltas (rollup) | `crates/medulla-server-cdc/src/reasoning_cascade.rs::tests::retraction_updates_closure_without_full_rebuild` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.1` | Monotone axiom insert is incremental | `crates/medulla-reasoner/src/owl.rs::tests::incremental_add_axiom_only_adds` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.2` | Writes on opted-in graphs refresh the closure | `crates/medulla-server-cdc/src/reasoning_cascade.rs::tests::opted_in_graph_axiom_write_makes_a_new_inference_appear_after_debounce` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.3` | A retraction never serves a stale entailment | `crates/medulla-server-cdc/src/reasoning_cascade.rs::tests::a_retraction_rebuilds_instead_of_serving_a_stale_entailment` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.4` | Model: a reverse index from causes to derived subsumptions | `crates/medulla-reasoner/src/owl.rs::tests::dependents_index_matches_justifications` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.5` | Behavior: delete and re-derive | `crates/medulla-reasoner/src/owl.rs::tests::retract_equals_fresh_run_randomized` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.6` | Wiring: the cascade retracts instead of rebuilding | `crates/medulla-server-cdc/src/reasoning_cascade.rs::tests::retraction_updates_closure_without_full_rebuild` |
| `MEDULLA-INCREMENTAL-VIEWS-R011.7` | Hot-path proof for retraction | `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::ontology_retraction_updates_closure_incrementally` |
| `MEDULLA-INCREMENTAL-VIEWS-R012` | SQL views gain DISTINCT, min and max (rollup) | `tests/pgwire_roundtrip.rs::matview_min_max_distinct_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R012.1` | Model: extreme aggregates and distinct | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::distinct_and_extremes_serialize` |
| `MEDULLA-INCREMENTAL-VIEWS-R012.2` | Behavior: insert updates, extreme retraction recomputes the group | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::retracting_the_minimum_recomputes_only_that_group` |
| `MEDULLA-INCREMENTAL-VIEWS-R012.3` | Wiring: the compiler accepts them | `crates/medulla-sql-lang/src/sql/matview_compile.rs::tests::distinct_min_max_compile` |
| `MEDULLA-INCREMENTAL-VIEWS-R012.4` | Oracle cases | `crates/medulla-table-store/tests/matview_oracle.rs::extremes_and_distinct_agree_after_every_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R012.5` | Served proof | `tests/pgwire_roundtrip.rs::matview_min_max_distinct_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R013` | SQL views over a two-table inner equi-join (rollup) | `tests/pgwire_roundtrip.rs::matview_join_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R013.1` | Model: join program | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::join_program_round_trips` |
| `MEDULLA-INCREMENTAL-VIEWS-R013.2` | Behavior: the join delta from post-commit tables | `crates/medulla-table-store/src/tables/matview_incremental.rs::tests::join_delta_equals_rejoin_for_both_sides` |
| `MEDULLA-INCREMENTAL-VIEWS-R013.3` | Wiring and refusal: the compiler accepts one indexed equi-join | `crates/medulla-sql-lang/src/sql/matview_compile.rs::tests::join_compiles_only_when_both_keys_are_indexed` |
| `MEDULLA-INCREMENTAL-VIEWS-R013.4` | Oracle case | `crates/medulla-table-store/tests/matview_oracle.rs::join_agrees_after_every_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R013.5` | Served proof | `tests/pgwire_roundtrip.rs::matview_join_after_commit` |
| `MEDULLA-INCREMENTAL-VIEWS-R014` | Documentation and quickstart | `tests/docs/test_incremental_views_doc.py::test_page_matches_knobs_codes_and_metrics` |
| `MEDULLA-INCREMENTAL-VIEWS-R015` | Release gate | `crates/medulla-plan/tests/incremental_oracle_coverage.rs::every_accepted_construct_has_an_oracle_case` |

## Negative and boundary cases

- A plan with each unsupported op kind refuses with its op index (R001.2).
- A consolidated delta that overflows refuses and leaves state untouched (R002.2, R002.3).
- A SQL query with each refused construct names that construct (R009.2).
- A NULL filter result drops the row; a NULL aggregate argument changes no sum (R009.1, R009.4).
- A group that reaches count zero leaves the view (R009.4).
- A rolled-back transaction leaves the view unchanged (R009.3).
- A shadow check during a concurrent write skips and re-arms (R008.2).
- A check that passes 30 s counts as skipped (R008.3).
- A `CdcRead` for an unknown view refuses `MATVIEW_UNKNOWN` (R010.3).
- A caller without read access to the view graph sees no status row and no feed (R005.3, R010.3).
- A join fan-out above the knob refuses the commit with `STATE_BUDGET_EXCEEDED` (R013.2).

## Quality and release proof

- Focused: `cargo test -p medulla-exec -p medulla-plan -p medulla-server-cdc -p medulla-table-store -p medulla-sql-lang --locked`.
- Served: `cargo test -p medulla-server-served-tests --features matview,redb incremental_views`.
- pgwire: `cargo test --test pgwire_roundtrip matview_`.
- Docs: `uv run pytest tests/docs/test_incremental_views_doc.py`.
- Nightly: `cargo bench -p medulla-plan --bench incremental_delta`, `cargo bench -p medulla-table-store --bench matview_incremental`, `cargo bench -p medulla-reasoner --bench owl_retract`.
- Scanners: the changed-file complexity, clone and KISS hooks of `.config/pre-commit.yaml` at manual stage, and `cargo clippy --no-default-features --features full --all-targets -- -D warnings`.
- Record the revision, the command and the exit status of each run in the pull request.
