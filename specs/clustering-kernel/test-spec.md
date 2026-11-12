# MEDULLA-CLUSTER — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Integration (L2) | Scan `crates/` for training function names outside the kernel. | The universe is non-empty and the finding list is empty. |  |
| T-002 | FR-002 | Integration (L2) | Three Gaussian blobs, 3,000 rows, `k = 3`, default plan; and `n = 500` for exact mode. | Centroids lie within 0.2 of the blob means; exact mode equals a brute-force Lloyd oracle. |  |
| T-003 | FR-003 | Integration (L2) | Same seed and data on pools of 1, 2 and 8 threads; two `IvfPq::train` runs. | Centroid and label bytes are equal; codebook bytes are equal. |  |
| T-004 | FR-004 | Unit (L1) | Enumerate every `State x Event` pair; run with a recording sink; cancel mid-run. | Each pair has a decided result; the run visits states in table order; a cancelled run returns no centroids. |  |
| T-005 | FR-005 | Integration (L2) | `k = 0`, `k > n`, empty input, unequal rows, NaN at row 3 column 2, duplicate rows, all-equal rows, `max_steps = 5` on noise. | Typed codes; the NaN refusal names row 3 and column 2; the noise run ends `BudgetExhausted`; no input runs past the budget. |  |
| T-006 | FR-006 | Integration (L2) | Counting allocator, `k = 8192`, `d = 64`; budget below the formula. | Peak live bytes stay within `training_bytes` plus 64 KiB; the low budget refuses before allocating. |  |
| T-007 | FR-007 | Integration (L2) | Scan `.cargo/config.toml`, kernel sources and the manifest; force the baseline and the wide distance path. | No FFI or CPU pin; both paths give equal bits. |  |
| T-008 | FR-008 | Served (L3) | `DsKMeans` on two blobs, then `k = 5` on 2 rows; SQL `kmeans`; spectral partition tests; `gmm` tests. | One label per blob and equal labels on repeat; `CLUSTERING_K_EXCEEDS_ROWS: ...`; existing tests pass. |  |
| T-009 | FR-009 | Served (L3) | Build an index on one distribution, add 5,000 vectors from a shifted one, run the sweep. | Drift ratio passes 1.5; one background retrain starts; queries keep answering; the swap installs the new codebook. |  |
| T-010 | FR-010 | Bench (L6) | `recall` bench in both modes at 100,000 vectors. | Build time, recall, inertia and peak memory printed; thresholds of R010.2 hold. |  |
| T-011 | FR-011 | Served (L3) | Hold a build mid-training; write from another task; query; shut down; boot with 100,000 embeddings. | Writer lock within 50 ms; exact answer within 2 s; shutdown yields `CLUSTERING_CANCELLED`; readiness precedes the build. |  |

## Row bindings

Each row names its proving test. A shipped row is bound by a `// spec: <ID>` line above its test. A test marked planned does not exist yet; its row gets the line when the test lands.

| Requirement | Name | Proving test |
|---|---|---|
| `MEDULLA-CLUSTER-R001` | One kernel owns centroid training (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_is_the_only_trainer` (planned) |
| `MEDULLA-CLUSTER-R001.1` | Module and types | `crates/medulla-numeric/tests/cluster_kernel.rs::rows_view_validates_shape` (planned) |
| `MEDULLA-CLUSTER-R001.2` | Typed refusals | `crates/medulla-numeric/tests/cluster_kernel.rs::every_refusal_has_a_distinct_code` (planned) |
| `MEDULLA-CLUSTER-R001.3` | Wiring: the served data-science method runs the kernel | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::ds_kmeans_served_separates_blobs_and_refuses_k_above_rows` (planned) |
| `MEDULLA-CLUSTER-R001.4` | Proof: only the kernel trains | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_is_the_only_trainer` (planned) |
| `MEDULLA-CLUSTER-R002` | E-means with scaling-law defaults (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::emeans_recovers_blobs_with_default_plan` (planned) |
| `MEDULLA-CLUSTER-R002.1` | Scaling law | `crates/medulla-numeric/src/cluster/plan.rs::tests::scaling_law_matches_table` (planned) |
| `MEDULLA-CLUSTER-R002.2` | Seeding | `crates/medulla-numeric/tests/cluster_kernel.rs::seeding_is_deterministic_and_distinct` (planned) |
| `MEDULLA-CLUSTER-R002.3` | Mini-batch moving-average update | `crates/medulla-numeric/tests/cluster_kernel.rs::moving_average_matches_reference_fold` (planned) |
| `MEDULLA-CLUSTER-R002.4` | Exact mode | `crates/medulla-numeric/tests/cluster_kernel.rs::exact_mode_matches_brute_force_lloyd_oracle` (planned) |
| `MEDULLA-CLUSTER-R002.5` | Every tunable is a knob | `crates/medulla-decision-stats/src/decision/cluster_knobs.rs::tests::defaults_lie_inside_bounds_and_equal_kernel_defaults` (planned) |
| `MEDULLA-CLUSTER-R002.6` | Wiring: the engine resolves knobs once per build | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::activation_uses_pinned_cluster_knobs` (planned) |
| `MEDULLA-CLUSTER-R003` | Bit-identical results (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::centroids_bit_identical_across_threads` (planned) |
| `MEDULLA-CLUSTER-R003.1` | Fixed reduction order | `crates/medulla-numeric/tests/cluster_kernel.rs::tie_goes_to_lowest_index_and_fold_is_row_ordered` (planned) |
| `MEDULLA-CLUSTER-R003.2` | Thread-count independence | `crates/medulla-numeric/tests/cluster_kernel.rs::centroids_bit_identical_across_threads` (planned) |
| `MEDULLA-CLUSTER-R003.3` | Index codebooks reproduce | `crates/medulla-ann/tests/ivfpq_kernel.rs::codebooks_are_byte_identical_across_runs_and_threads` (planned) |
| `MEDULLA-CLUSTER-R004` | Explicit run state machine (rollup) | `crates/medulla-numeric/src/cluster/state.rs::tests::run_visits_states_in_table_order` (planned) |
| `MEDULLA-CLUSTER-R004.1` | Transition table | `crates/medulla-numeric/src/cluster/state.rs::tests::legal_transitions_follow_table` (planned) |
| `MEDULLA-CLUSTER-R004.2` | Every transition is tested | `crates/medulla-numeric/src/cluster/state.rs::tests::every_state_event_pair_is_decided` (planned) |
| `MEDULLA-CLUSTER-R004.3` | Consumer: progress reaches the background-task registry | `crates/medulla-numeric/tests/cluster_kernel.rs::progress_is_reported_per_state_change_and_throttled` (planned) |
| `MEDULLA-CLUSTER-R004.4` | Cancellation | `crates/medulla-numeric/tests/cluster_kernel.rs::cancel_stops_at_next_boundary_without_centroids` (planned) |
| `MEDULLA-CLUSTER-R005` | Bounded work (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::no_input_spins` (planned) |
| `MEDULLA-CLUSTER-R005.1` | Input refusals | `crates/medulla-numeric/tests/cluster_kernel.rs::bad_inputs_refuse_with_typed_codes` (planned) |
| `MEDULLA-CLUSTER-R005.2` | Step budget | `crates/medulla-numeric/tests/cluster_kernel.rs::budget_exhaustion_is_a_named_end_state` (planned) |
| `MEDULLA-CLUSTER-R005.3` | Deterministic empty-cluster reseed | `crates/medulla-numeric/tests/cluster_kernel.rs::empty_cluster_reseeds_farthest_point_lowest_index_first` (planned) |
| `MEDULLA-CLUSTER-R005.4` | Too few distinct rows | `crates/medulla-numeric/tests/cluster_kernel.rs::duplicate_rows_refuse_or_clamp` (planned) |
| `MEDULLA-CLUSTER-R006` | Bounded training memory (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::peak_memory_within_formula` (planned) |
| `MEDULLA-CLUSTER-R006.1` | Formula | `crates/medulla-numeric/src/cluster/plan.rs::tests::training_bytes_has_no_k_d_threads_term` (planned) |
| `MEDULLA-CLUSTER-R006.2` | Refuse before allocation | `crates/medulla-numeric/tests/cluster_kernel.rs::over_budget_refuses_before_allocating` (planned) |
| `MEDULLA-CLUSTER-R006.3` | Measured peak | `crates/medulla-numeric/tests/cluster_memory.rs::peak_live_bytes_within_formula` (planned) |
| `MEDULLA-CLUSTER-R007` | Baseline instruction set, native Rust (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_has_no_ffi_and_no_target_cpu_flags` (planned) |
| `MEDULLA-CLUSTER-R007.1` | No compile-time CPU pin, no FFI | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_has_no_ffi_and_no_target_cpu_flags` (planned) |
| `MEDULLA-CLUSTER-R007.2` | Runtime-detected wide path | `crates/medulla-numeric/tests/cluster_kernel.rs::wide_path_is_bit_equal_to_baseline` (planned) |
| `MEDULLA-CLUSTER-R008` | Every caller runs on the kernel (rollup) | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_is_the_only_trainer` (planned) |
| `MEDULLA-CLUSTER-R008.1` | IVF coarse quantizer and PQ codebooks train on the kernel | `crates/medulla-ann/tests/ivfpq_kernel.rs::recall_matches_exact_mode_within_budget` (planned) |
| `MEDULLA-CLUSTER-R008.2` | Distance helpers move to the kernel | `crates/medulla-numeric/tests/cluster_kernel.rs::sq_dist_matches_scalar_reference_bits` (planned) |
| `MEDULLA-CLUSTER-R008.3` | The accelerator becomes a kernel backend | `crates/medulla-numeric/tests/cluster_kernel.rs::custom_backend_is_used_and_must_agree` (planned) |
| `MEDULLA-CLUSTER-R008.4` | The served data-science method | `crates/medulla-compute/src/datascience/primitives.rs::tests::test_kmeans` |
| `MEDULLA-CLUSTER-R008.5` | The SQL aggregate and the Python kernel | `crates/medulla-numeric/tests/cluster_kernel.rs::python_surface_args_map_to_config` (planned) |
| `MEDULLA-CLUSTER-R008.6` | Spectral partitions (MEDULLA-FEDERATED-QUERY-R607.2) | `crates/medulla-graph-algos/src/graph_algos/spectral.rs::tests::planted_partition_is_recovered_and_ids_are_stable` |
| `MEDULLA-CLUSTER-R008.7` | Mixture-model seeding | `crates/medulla-mining/src/mining/cluster.rs::tests::gmm_is_deterministic_for_a_seed` |
| `MEDULLA-CLUSTER-R008.8` | Sweep for leftovers | `crates/medulla-numeric/tests/cluster_kernel.rs::kernel_is_the_only_trainer` (planned) |
| `MEDULLA-CLUSTER-R009` | Online centroid refresh with drift-triggered retrain (rollup) | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::shifted_ingest_triggers_one_background_retrain` (planned) |
| `MEDULLA-CLUSTER-R009.1` | Centroid tracker | `crates/medulla-numeric/tests/cluster_kernel.rs::tracker_follows_shifted_distribution` (planned) |
| `MEDULLA-CLUSTER-R009.2` | Drift ratio | `crates/medulla-numeric/tests/cluster_kernel.rs::drift_ratio_crosses_threshold_only_after_warmup` (planned) |
| `MEDULLA-CLUSTER-R009.3` | Wiring: ingest feeds the tracker | `crates/medulla-semantic-index/src/semantic_store_ann.rs::tests::insert_after_shift_sets_stale_flag` (planned) |
| `MEDULLA-CLUSTER-R009.4` | Background retrain, warm start, swap | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::shifted_ingest_triggers_one_background_retrain` (planned) |
| `MEDULLA-CLUSTER-R010` | Benchmark against the current full-batch method (rollup) | `crates/medulla-ann/tests/ivfpq_kernel.rs::recall_matches_exact_mode_within_budget` (planned) |
| `MEDULLA-CLUSTER-R010.1` | Bench harness | `crates/medulla-ann/tests/ivfpq_kernel.rs::bench_harness_modes_parse` (planned) |
| `MEDULLA-CLUSTER-R010.2` | Acceptance thresholds | `crates/medulla-ann/tests/ivfpq_kernel.rs::recall_matches_exact_mode_within_budget` (planned) |
| `MEDULLA-CLUSTER-R011` | A build never stalls the engine (rollup) | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::build_held_midway_does_not_block_writers_or_queries` (planned) |
| `MEDULLA-CLUSTER-R011.1` | Build off the store locks | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::build_held_midway_does_not_block_writers_or_queries` (planned) |
| `MEDULLA-CLUSTER-R011.2` | Writer latency bound | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::writer_waits_at_most_50ms_during_build` (planned) |
| `MEDULLA-CLUSTER-R011.3` | Bounded compute pool | `crates/medulla-numeric/tests/cluster_kernel.rs::fit_uses_its_own_pool_not_the_global_one` (planned) |
| `MEDULLA-CLUSTER-R011.4` | Shutdown cancels the build | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::shutdown_cancels_running_build` (planned) |
| `MEDULLA-CLUSTER-R011.5` | Readiness does not wait for the build | `crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs::ready_before_index_build_finishes` (planned) |
