# MEDULLA-CLUSTER — Data model

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Kernel types (R001.1)

Directory: `crates/medulla-numeric/src/cluster/`.

| Type | Field | Type | Rule |
|---|---|---|---|
| `Rows<'a>` | `data` | `&'a [f32]` | Row-major, `n * dim` values |
| `Rows<'a>` | `n`, `dim` | `usize` | Both at least one |
| `ClusterConfig` | `seed` | `u64` | Default 42 (`KMEANS_DEFAULT_SEED`) |
| `ClusterConfig` | `exact` | `bool` | Forces exact mode |
| `ClusterConfig` | `at_most_k` | `bool` | Clamp `k` instead of refusing |
| `ClusterConfig` | `labels` | `bool` | Label every row after training; default true |
| `ClusterConfig` | `warm_start` | `Option<Vec<f32>>` | `k * dim` seeds that replace seeding (R009.4) |
| `ClusterConfig` | knob fields | see the knob table | One field per knob, same names without the family |
| `ClusterOutcome` | `centroids` | `Vec<f32>` | `k * dim` |
| `ClusterOutcome` | `labels` | `Vec<u32>` | One per row; ties to the lowest index |
| `ClusterOutcome` | `end` | `End` | `Converged` or `BudgetExhausted` |
| `ClusterOutcome` | `steps`, `inertia`, `mean_sq_dist` | `u32`, `f64`, `f64` | Sum and mean of squared distances; exact over all rows when labels are requested, else the smoothed batch estimate |
| `ClusterRefusal` | variants | enum | Eight variants, codes in R001.2 |
| `CancelToken` | inner | `Arc<AtomicBool>` | Checked at step boundaries |
| `Progress` | `state`, `step`, `max_steps`, `shift`, `elapsed_ms` | scalars | Sent to a `ProgressSink` |

## Plan (R002.1)

| Field | Type | Rule |
|---|---|---|
| `batch` | `usize` | `clamp(32*k, 1024, batch_max_rows)`, lowered to fit `step_budget_madds`, at least `min(n, 256)` |
| `steps` | `u32` | `clamp(ceil(epochs * n / batch), min_steps, max_steps)` |
| `mode` | `Mode` | `Exact` when `n <= batch`, else `EMeans` |

## Knob table (R002.5)

File: `crates/medulla-decision-stats/src/decision/cluster_knobs.rs`. Values are `Integer` unless stated.

| Name | Unit | Default | Min | Max | Safety |
|---|---|---|---|---|---|
| `cluster.batch_rows` | rows | 0 (auto) | 0 | 1,048,576 | Tunable |
| `cluster.batch_max_rows` | rows | 65,536 | 1,024 | 1,048,576 | Tunable |
| `cluster.epochs` | passes (`DecimalMicros`) | 3.0 | 0.5 | 50.0 | Tunable |
| `cluster.step_budget_madds` | multiply-adds | 2,147,483,648 | 1,000,000 | 1,000,000,000,000 | Tunable |
| `cluster.min_steps` | steps | 8 | 1 | 1,000 | Tunable |
| `cluster.max_steps` | steps | 300 | 1 | 10,000 | Fixed |
| `cluster.tol` | relative inertia change (`DecimalMicros`) | 0.001 | 0.000001 | 0.1 | Tunable |
| `cluster.patience` | steps | 3 | 1 | 50 | Tunable |
| `cluster.empty_patience` | steps | 2 | 1 | 20 | Tunable |
| `cluster.ema_window` | points | 8,192 | 64 | 100,000,000 | Tunable |
| `cluster.plusplus_max_k` | cells | 4,096 | 2 | 65,536 | Tunable |
| `cluster.memory_budget_bytes` | bytes | 1,073,741,824 | 1,048,576 | 68,719,476,736 | Tunable |
| `cluster.threads` | threads (0 = half the cores) | 0 | 0 | 256 | Tunable |
| `cluster.refresh_drift_ratio` | ratio (`DecimalMicros`) | 1.5 | 1.05 | 10.0 | Tunable |
| `cluster.online_window` | points | 4,096 | 64 | 100,000,000 | Tunable |
| `cluster.snapshot_chunk_rows` | rows | 4,096 | 256 | 1,048,576 | Tunable |

The kernel holds the same defaults as constants in `config.rs`. A test asserts the table and the constants agree.

## State machine (R004.1)

| From | Event | To |
|---|---|---|
| `Seeding` | `SeedsReady` | `Sampling` |
| `Seeding` | `Refuse(r)` | `Refused(r)` |
| `Sampling` | `BatchDrawn` | `Updating` |
| `Sampling` | `Refuse(r)` | `Refused(r)` |
| `Updating` | `StepDone { converged: true, .. }` | `Converged` |
| `Updating` | `StepDone { converged: false, budget_left: true }` | `Sampling` |
| `Updating` | `StepDone { converged: false, budget_left: false }` | `BudgetExhausted` |
| `Updating` | `Refuse(r)` | `Refused(r)` |
| `Converged`, `BudgetExhausted`, `Refused(_)` | any | `Err(StateIllegal)` |

Every other pair returns `StateIllegal`. Exact mode passes through `Sampling` with the whole input as its batch.

## Memory formula (R006.1)

`training_bytes = 4*k*d + 120*k + 32768 + 16*batch + 8*d`.

## Tracker (R009)

| Type | Field | Type | Rule |
|---|---|---|---|
| `CentroidTracker` | `centroids` | `Vec<f32>` | Copy of the built codebook |
| `CentroidTracker` | `counts` | `Vec<u64>` | Capped at `online_window` |
| `CentroidTracker` | `training_inertia` | `f64` | From the run that built the index |
| `CentroidTracker` | `ingest_inertia` | `f64` | Exponential average over `online_window` |
| `CentroidTracker` | `observed` | `u64` | Rows seen since the build |

## Build journal (R011.1)

| Type | Field | Type | Rule |
|---|---|---|---|
| `BuildJournal` | `adds` | `Vec<(String, Vec<f32>)>` | Embedding writes since the snapshot, in order |
| `BuildJournal` | `removes` | `Vec<String>` | Node removals since the snapshot, in order |
| `BuildJournal` | `base_generation` | `u64` | Store generation at the snapshot |
