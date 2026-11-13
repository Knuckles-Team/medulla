# MEDULLA-CLUSTER — Research

Governing spec: [spec.md](spec.md). Plan: [plan.md](plan.md).

Each entry states a decision, its rationale and the alternatives considered. All findings are in this platform's terms.

## Techniques

- **Distance-squared seeding.** Pick the first centroid at random. Pick each next centroid with probability proportional to its squared distance to the nearest chosen centroid. Seeds spread out and the run needs fewer steps.
- **Mini-batch updates.** Each step uses a random batch instead of every row. The cost of a step follows the batch size, not the row count.
- **Moving-average centroids.** A centroid moves toward the mean of its batch points by a rate that falls with its point count and never drops below a floor. The floor keeps a tracker responsive to new data.
- **Exact full-batch method.** Every step assigns every row and sets each centroid to its cluster mean. It is the reference for quality.
- **Fixed-order reduction.** Floating-point sums depend on order. A fold in a fixed order gives the same bits on any thread count.
- **Drift ratio.** The mean squared distance of new rows to their centroids, divided by the same value at training time. A rise means the data moved away from the codebook.

## Decisions

### D1. Owner and placement: `medulla-numeric`

- Decision: the kernel lives in `crates/medulla-numeric/src/cluster/`. The knob table lives in `medulla-decision-stats`, which already depends on `medulla-numeric` and on the knob types.
- Evidence: `medulla-numeric` depends only on `medulla-version`. The crate layer file puts `medulla-numeric`, `medulla-ann`, `medulla-semantic-index` and `medulla-decision-stats` in the same `kernels` layer, and `medulla-compute`, `medulla-graph-algos`, `medulla-mining`, `medulla-table-store` and `medulla-sql` in the later `engine` layer. Every caller may therefore depend on it with no cycle and no layer break.
- Alternative considered: keep the kernel in `medulla-ann` and let others depend on it. Rejected because `medulla-ann` is an index crate and the data-science and graph callers would depend on an index to cluster rows.
- Alternative considered: a new crate. Rejected; the kernel is about 1,500 lines and `medulla-numeric` already owns the previous kernel (anti-sprawl).
- The knob table cannot live in `medulla-numeric`, because `KnobSpec` is in `medulla-decision-types`, which depends on eight type crates. The kernel holds the default constants. A test asserts that the table equals them.

### D2. Element type: `f32` rows, `f64` accumulation

- Decision: the kernel takes `f32` rows and sums in `f64`.
- Evidence: the index stores `f32`. The served method and the spectral code hold `f64` rows. Cluster labels do not depend on the low bits of the input.
- Consequence: f64 callers convert once per run. `KMeansResult.centroids` converts back to `f64`.
- Alternative considered: a generic kernel over both widths. Rejected; it doubles the monomorphised code for no change in labels.

### D3. Determinism without a deterministic reduction library

- Finding: the current loop folds per-thread `f64` partials with `rayon` `fold` and `reduce` (`crates/medulla-ann/src/kmeans.rs`). The split points of those partials depend on the thread count, so float sums can differ between hosts.
- Decision: assignment is a per-row map. The fold of batch rows into centroids is sequential in row order. Its cost is `batch * d` adds against `batch * k * d` multiply-adds for assignment, so it is under `1/k` of a step.
- Alternative considered: fixed-size blocks with ordered partials. Rejected; it needs `k*d` memory per block and breaks the large-`k` bound of R006.

### D4. Seeding at large k

- Finding: distance-squared seeding costs `s * k * d` for a sample of `s` rows. At `k = 65,536` and `d = 128` with `s = 4,096` that is 3.4e10 multiply-adds, which is too slow for a boot build.
- Decision: use it up to `cluster.plusplus_max_k` (4,096) over a sample of `max(8 * k, 4096)` rows. Above that, take `k` distinct seeded rows. The mini-batch steps repair a rough seed.

### D5. Scaling law

- Batch: `clamp(32 * k, 1024, batch_max_rows)`. Each centroid sees about 32 points per step on average, which keeps the per-step centroid noise small. Then the batch drops so `batch * k * d` stays under the step budget (2.1e9 multiply-adds, about 0.5 s on one baseline core), so a large `k` or `d` still yields steps that can be cancelled within 50 ms checks.
- Steps: `ceil(epochs * n / batch)` with three passes, clamped to 8..300. Three epochs reach the inertia plateau on the bench data in the measurement of D7.
- Exact mode when `n <= batch`: a sample that small costs less per step than the mini-batch machinery.
- The numbers are starting defaults. The D7 measurement replaces them where it disagrees.

### D6. The accelerator

- Evidence: `kmeans_gpu.rs` has one consumer, the assignment loop of the old `kmeans` (`kmeans.rs` calls `batch_assign_dispatch`). The `gpu-cuda` feature is outside `default`; it appears in `full-extras` only. The CPU implementation in that file duplicates `nearest_centroid`.
- Decision: keep the CUDA implementation as a backend of the kernel (R008.3) and delete the duplicated CPU dispatch. The backend contract: same label as the CPU path for every row.
- Alternative considered: delete the accelerator. Rejected; `full-extras` ships it. Deleting a shipped capability needs an owner decision, not a refactor.
- Alternative considered: move the CUDA code into `medulla-numeric`. Rejected; it would add `cudarc` to the lowest numeric crate.

### D7. Baseline and acceptance thresholds

- Method: the `recall` bench runs at 100,000 clustered vectors, `d = 128`, 1,024 coarse cells, 32 subquantizers, 32 probes and 200 queries, once on the full-batch method before R008.1 lands and once per mode after. It reports build time, recall@10, inertia and peak resident memory.
- Starting thresholds: build time at most 0.33 times, recall@10 at most 0.5 points lower, inertia at most 1.03 times. The first two follow from the goal that the mini-batch mode saves two thirds of the build at no visible recall cost. The third bounds the quality loss that recall alone may hide.
- Why these are defensible: recall@10 over 200 queries has a standard error near 0.3 points at recall 0.9, so a 0.5-point gap is the smallest difference the bench can tell from noise. Inertia is deterministic for a fixed seed, so 3 percent is a direct quality bound.
- Rule: the measured values replace these thresholds in R010.2 when the bench disagrees. Slice C of [tasks.md](tasks.md) records the measured numbers in this entry.

### D8. The run machine, the pinned policy and the registry sink

- Decision (machine): `ClusterRunMachine` uses the core machine contract (`MachineDef`, `transition`, `TransitionRefused`) from `medulla-types`. `State` is `Copy`; `Refused` carries no payload because the cause is the refusal `fit` returns. The three `StepDone` edges share one event and differ by guards, so one event name covers the "converged, budget left, budget spent" outcomes the row lists. `medulla-types` becomes a normal dependency of `medulla-numeric`: it is already linked through `medulla-protocol-types`, so the change adds no crate.
- Decision (pinned policy): the engine keeps its pinned decision policy in `ServerState.pinned_policy`. The semantic index build resolves clustering knobs from it once, before the build starts, and falls back to the knob defaults when it is `None`. No per-graph policy exists, so the one engine policy governs every graph.
- Decision (progress): the index build reports to a log sink until the `MEDULLA-OBSERVE-R003` registry crate exists; the sink is a parameter of `activate_with` and `sweep_resident_graphs_with`, so binding the registry (task T056) replaces one argument.
