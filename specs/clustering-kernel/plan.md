# MEDULLA-CLUSTER — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

Six crates train centroids today with five separate loops. Each loop has its own seeding, its own empty-cluster rule and its own stopping rule. None is bounded by a typed budget, and the index loop is not reproducible across thread counts. This plan collapses them into one kernel in `medulla-numeric`. The kernel trains with seeded mini-batch updates and moving-average centroids (e-means). It runs the exact full-batch method as a mode. It adds a run state machine, typed refusals, a memory formula, online drift tracking and a build that holds no store lock. Every caller moves in the same change that deletes its old copy.

## Technical Context

- **Language/Version**: Rust, toolchain of `rust-toolchain.toml`.
- **Primary Dependencies**: `rayon` (new for `medulla-numeric`), `rand`, `rand_chacha` (already present). No other dependency.
- **Storage**: none. Codebooks persist through the existing durable code store.
- **Testing**: unit (L1), integration through the public kernel API (L2), served dispatch tests (L3), benches (L6).
- **Target Platform**: x86-64 baseline (no AVX2 required), arm64, Linux, macOS and Windows wheels.
- **Project Type**: library crates in one workspace.
- **Performance Goals**: build at most 0.33 times exact-mode time at 100,000 vectors; recall@10 within 0.5 points.
- **Constraints**: no FFI, no compile-time CPU pin, no float reduction in `rayon`, every wait bounded.
- **Scale/Scope**: 56 rows. Sixteen knobs. Nine caller sites. About 1,900 lines deleted, about 1,500 added.

## Constitution Check

- I. One owner per concept: pass. This spec owns centroid training. R607.2, R720.3, R040.2 and R175.2 stay with their owners and call the kernel.
- II. Extend before you create: pass. The search is in spec.md "Why a new spec".
- III. Specs describe the current state only: pass.
- IV. Requirements are neutral and controlled: pass. No product names.
- V. Hard cutover, no legacy path: pass. R008 deletes each old copy with its caller change. No re-export, no alias.
- VII. One engine, no second database: pass. No store is added.
- XI. Code stays small: pass. The kernel is nine files under 900 lines each.
- XII. No ratchets and no suppressions: pass.
- XIII. Zero flakes: pass. Tests use fixed seeds and bounded waits.
- XIV. Compile first: pass. Tasks order the producer before the callers.
- XVIII. Landed means reachable: pass. R001.3, R008 and R011 have served proofs.
- XXIV. Complete specifications: pass. Rows name chain, files, types, codes, knobs and tests.
- XXV. Small, modular units: pass. `medulla-ann`, `medulla-semantic-index`, `medulla-compute`, `medulla-graph-algos` and `medulla-mining` gain an edge to `medulla-numeric`, which sits in the same or an earlier layer.
- R-I. Native Rust, only fact store: pass.
- R-II. Every wait has a deadline: pass. R011.2 bounds the writer wait; R004.4 bounds cancel.
- R-III. Budgets and a benchmark per row: pass. Rows state budgets and name a bench.
- R-IV. Crates are small, layered, cut over hard: pass. `scripts/check_crate_graph.py` layers hold; see Project Structure.

## Project Structure

```text
crates/medulla-numeric/src/cluster.rs                       existing, replaced by the directory (R001.1)
crates/medulla-numeric/src/cluster/mod.rs                   new (R001.1, R005.1, R005.4)
crates/medulla-numeric/src/cluster/config.rs                new (R002.5)
crates/medulla-numeric/src/cluster/plan.rs                  new (R002.1, R006.1)
crates/medulla-numeric/src/cluster/seed.rs                  new (R002.2)
crates/medulla-numeric/src/cluster/distance.rs              new (R008.2, R007.2)
crates/medulla-numeric/src/cluster/backend.rs               new (R008.3)
crates/medulla-numeric/src/cluster/train.rs                 new (R002.3, R002.4, R003, R005.3)
crates/medulla-numeric/src/cluster/state.rs                 new (R004)
crates/medulla-numeric/src/cluster/refusal.rs               new (R001.2)
crates/medulla-numeric/src/cluster/online.rs                new (R009.1, R009.2)
crates/medulla-numeric/tests/cluster_kernel.rs              new (kernel proofs)
crates/medulla-numeric/tests/cluster_memory.rs              new (R006.3)
crates/medulla-numeric/benches/cluster_kernel.rs            new (R010.1)
crates/medulla-numeric/src/py.rs                            existing (R008.5)
crates/medulla-decision-stats/src/decision/cluster_knobs.rs new (R002.5)
crates/medulla-ann/src/kmeans.rs                            existing, deleted (R008.2)
crates/medulla-ann/src/kmeans_gpu.rs                        existing, reduced to the CUDA backend (R008.3)
crates/medulla-ann/src/ivfpq.rs                             existing (R008.1)
crates/medulla-ann/src/{lib,flat,recall,durable_codes}.rs   existing (R008.1, R008.2)
crates/medulla-ann/benches/recall.rs                        existing (R010.1)
crates/medulla-ann/tests/ivfpq_kernel.rs                    new (R003.3, R010.2)
crates/medulla-semantic-index/src/semantic_ann.rs           existing (R002.6, R008.1)
crates/medulla-semantic-index/src/semantic_ann_index.rs     existing (R011.1)
crates/medulla-semantic-index/src/semantic_ann_mutation.rs  existing (R009.3)
crates/medulla-semantic-index/src/semantic_store_ann.rs     existing (R008.2, R009.3)
crates/medulla-server-graph/src/semantic_activation.rs      existing (R002.6, R009.4, R011.3 to R011.5)
crates/medulla-table-store/src/tables/ann_authority/generation.rs existing (R008.1)
crates/medulla-sql/src/sql/ann.rs                           existing (R008.1)
crates/medulla-sql-udfs/src/numeric.rs                      existing (R008.5)
crates/medulla-compute/src/datascience/primitives.rs        existing (R008.4)
crates/medulla-h-compute/src/datascience/routes.rs          existing (R001.3)
crates/medulla-graph-algos/src/graph_algos/spectral.rs      existing (R008.6)
crates/medulla-mining/src/mining/cluster.rs                 existing (R008.7)
crates/medulla-server-served-tests/src/split_tests/clustering_kernel.rs new (served proofs)
src/main.rs                                                 existing (R011.4)
```

## Phase 0: Research

Seven unknowns were resolved in [research.md](research.md): the lowest legal owner, the element type, how to keep a parallel fold deterministic, what to do with the accelerator, why a boot build stalls the engine, which defaults the scaling law needs, and the acceptance thresholds.

## Phase 1: Design

### Pre-untangled design

One line per answer.

- **Owner.** Medulla `crates/medulla-numeric/src/cluster/`. Every caller holds a call to `fit` or `seed_centers`; none holds a loop.
- **Cohesion.** Seeding, planning, stepping, the state machine and the drift tracker share one `ClusterConfig` and one refusal type, so they live in one module directory.
- **Dependency direction.** Callers in `medulla-ann`, `medulla-compute`, `medulla-graph-algos`, `medulla-mining`, `medulla-sql-udfs` and `medulla-semantic-index` point down to `medulla-numeric`. The knob table sits in `medulla-decision-stats`, which already points at both the knob types and the kernel.
- **Interface depth.** Two entry points (`fit`, `seed_centers`), one config, one outcome, one refusal enum and one backend trait. The distance helpers are the only other public items.
- **Special cases.** None per caller. Exact mode is a value of `Mode`; a caller that needs full-batch sets `exact`.
- **State.** One run state machine with a typed table (R004). The tracker keeps counters and two averages. The index build keeps a journal until the swap.
- **Environment.** Sixteen knobs (R002.5) read once per build; the pool size comes from `cluster.threads`. The kernel reads no environment variable.

### Kernel

`fit(rows, k, cfg, sink, cancel)` validates, plans, seeds, then loops steps until an end state. The plan comes from `n`, `k`, `d` and the knobs. A step draws a batch with the seeded generator, assigns each row to its nearest centroid in parallel, and folds batch means into the centroids in row order. Exact mode uses the whole input as the batch and replaces centroids with means. See [data-model.md](data-model.md) for types.

### Determinism

Assignment writes one label per row, so row order cannot change it. The fold is sequential. Distances use the 8-lane order of `lane_reduce`. The pool size changes only how many rows are assigned at once. A wide-instruction path must keep the same lane order and use no fused multiply-add.

### Memory

Nothing in a step has a `k*d*threads` term, because there are no per-thread centroid accumulators. The formula in R006.1 is the contract and the counting-allocator test checks it.

### Build off the lock

The index build copies a bounded sample, trains with no lock, encodes in chunks under short read locks, journals writes that arrive meanwhile, and swaps under one short write lock. The cause of today's stall is in two places: `build_index` holds the index write lock for the whole build, and `activate_one` holds the store read lock across it. A waiting writer then blocks new readers.

### Failure modes

- Bad input: refuse before work (R005.1, R005.4).
- Over budget: refuse before allocation (R006.2).
- No convergence: end `BudgetExhausted` with the best centroids (R005.2).
- Shutdown: cancel; install nothing (R011.4).
- Build error: the store stays `STATE_COLD`; queries use exact search.

The served behavior and its commands are in [quickstart.md](quickstart.md). The progress seam is in [contracts/progress-sink.md](contracts/progress-sink.md).

## Phase 2: Task approach

`tasks.md` lists one task per row in dependency order: kernel and state machine (slice A), caller migration and deletions (slice B), online refresh, build safety and benchmark (slice C). The baseline of the old method is measured before slice B lands. Parallel tasks carry `[P]`.

## Complexity Tracking

none

## Build contract

Every change passes the hooks of `.config/pre-commit.yaml` at the pre-commit and pre-push stages.

| Gate | Threshold | Command |
|---|---|---|
| Format | rustfmt clean, edition 2021 | `cargo fmt --all -- --check` |
| Lint | zero warnings | `cargo clippy -p <crate> --all-targets -- -D warnings` for each touched crate and direct dependent |
| KISS (Rust) | statements per function 35; arguments 8; returns 8; indentation 5; locals 20; calls 45; lines per file 900; functions per file 40 | `pre-commit run kiss-changed-rust --hook-stage manual --config .config/pre-commit.yaml` |
| Clones | zero new pairs | `pre-commit run dupehound-changed-functions jscpd-differential --hook-stage manual --config .config/pre-commit.yaml` |
| Crate graph | layers hold | `python3 scripts/check_crate_graph.py` |
| Affected tests | default features, touched crates and direct dependents | `cargo nextest run --locked -p <crate>` |
| Spec | clean | `pipelines-hook spec-standard`, `pipelines-hook plan-conformance` |
| Wording | 20 words per sentence | `pipelines-hook ste-staged` |

Code shapes: the kernel files stay under 900 lines. No function takes more than 8 arguments, so `fit` takes a `FitArgs` struct. No new `#[allow]`. The `clippy.toml` ban on std transcendentals in `medulla-numeric` stays; the kernel needs only `+ - * /` and comparisons.

## Production caller chain

- Served method: `Method::DsKMeans` arm (`crates/medulla-h-compute/src/datascience/routes.rs:55`) → `primitives::kmeans` (`crates/medulla-compute/src/datascience/primitives.rs:152`) → `cluster::fit`.
- SQL aggregate: `kmeans_result` (`crates/medulla-sql-udfs/src/numeric.rs:642`) → `cluster::kmeans_labels` → `fit`.
- Python kernel: `kmeans` (`crates/medulla-numeric/src/py.rs:431`) → `fit`.
- Index build: `activate_one` (`crates/medulla-server-graph/src/semantic_activation.rs:69`) → `SemanticStore::warm` (`crates/medulla-semantic-index/src/semantic_ann_index.rs:8`) → `build_index` (`semantic_ann_index.rs:52`) → `AnnIndex::build` (`crates/medulla-semantic-index/src/semantic_ann.rs:73`) → `IvfPq::train` (`crates/medulla-ann/src/ivfpq.rs:140`) → `fit` (`ivfpq.rs:213`, `ivfpq.rs:586`).
- Table-store index: `generation.rs:264` → `IvfPq::train` → `fit`.
- SQL index: `crates/medulla-sql/src/sql/ann.rs:184` → `IvfPq::train` → `fit`.
- Spectral: `spectral_partition` (`crates/medulla-graph-algos/src/graph_algos/spectral.rs:153`) → `fit`.
- Mixture seeding: `gmm` (`crates/medulla-mining/src/mining/cluster.rs:284`) → `seed_centers`.
- Ingest: `SemanticStore::insert_embedding` (`semantic_ann_mutation.rs:51`) → `maintain_incremental_index` → `CentroidTracker::observe`.
- Boot and sweep: `src/main.rs:1718` → `sweep_resident_graphs` (`semantic_activation.rs:265`) → `spawn_activation` (`semantic_activation.rs:226`) → `spawn_blocking` → `activate_one`.

Obsolete paths to delete: `crates/medulla-ann/src/kmeans.rs` (R008.2); the Lloyd loop, `kmeans_plus_plus` and `lloyd_step` in the old `crates/medulla-numeric/src/cluster.rs` (R008.5); `assign_labels`, `update_centroids`, `cluster_inertia` in `primitives.rs` (R008.4); `kmeans`, `sq_dist` in `spectral.rs` (R008.6); `kmeanspp_init` in `mining/cluster.rs` (R008.7); `CpuBackend`, `batch_assign_dispatch`, `active_backend` in `kmeans_gpu.rs` (R008.3).

## Cross-repo seams

- No other repository is involved. The progress seam is inside medulla: `MEDULLA-OBSERVE` provides the background-task registry and `MEDULLA-CLUSTER-R004.3` consumes it. Provider row: not yet filed; task `T-OBSERVE-BIND` binds it. Contract: [contracts/progress-sink.md](contracts/progress-sink.md).
