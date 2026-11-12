# MEDULLA-CLUSTER — One clustering kernel

Owner: medulla. Sole owner of centroid clustering of numeric vectors: seeding, training, assignment helpers, run state, knobs and online centroid refresh.
Cross-repo IDs: none. Every seam of this spec is inside medulla. The progress seam consumes the background-task registry of `MEDULLA-OBSERVE`, specified in parallel in the same repository.

In this spec, *clustering* means grouping numeric vectors around centroids. It never means a group of engine nodes; replication and consensus keep their own `CLUSTER_` refusal codes.

## Delivery mapping

Component: Compute
Depends-on: MEDULLA-DECISION-ENGINE-R179
References: [MEDULLA-DECISION-ENGINE](../decision-engine/spec.md), [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md), [MEDULLA-INCREMENTAL-VIEWS](../incremental-views/spec.md)
Code: ../../crates/medulla-numeric/src/cluster/mod.rs ../../crates/medulla-numeric/src/cluster.rs ../../crates/medulla-decision-stats/src/decision/cluster_knobs.rs ../../crates/medulla-semantic-index/src/semantic_ann_index.rs ../../crates/medulla-ann/src/kmeans.rs ../../crates/medulla-ann/src/kmeans_gpu.rs ../../crates/medulla-ann/src/ivfpq.rs ../../crates/medulla-compute/src/datascience/primitives.rs ../../crates/medulla-graph-algos/src/graph_algos/spectral.rs ../../crates/medulla-mining/src/mining/cluster.rs ../../crates/medulla-server-graph/src/semantic_activation.rs

## User Scenarios & Testing

### User Story 1 - Index builds finish fast and reproduce (Priority: P1)

An operator restarts the engine with a large embedding graph. The engine trains the vector index in the background with mini-batch updates. The build costs a fraction of a full pass per iteration. The same seed and data give the same index on every host and thread count.

**Acceptance Scenarios**:

1. **Given** 100,000 training vectors and 1,024 cells, **When** the engine trains the index twice with seed 42, **Then** both codebooks are byte-identical.
2. **Given** the same input, **When** the kernel runs on 1, 2 and 8 threads, **Then** the centroids are bit-identical.
3. **Given** the recall bench at 100,000 vectors, **When** the mini-batch mode replaces the full-batch mode, **Then** recall@10 drops by at most 0.5 points and training is at least 3 times faster.

### User Story 2 - A build never stalls the engine (Priority: P1)

A client queries the commons graph while the engine builds its vector index. The query answers by exact search within its normal budget. Writes to the graph are never blocked by the build.

**Acceptance Scenarios**:

1. **Given** a build held mid-training, **When** a writer takes the graph's embedding store for writing, **Then** it gets the lock within 50 ms.
2. **Given** a build held mid-training, **When** a client sends a served nearest-neighbour query, **Then** the engine answers by exact search within 2 s.
3. **Given** a running build, **When** the engine shuts down, **Then** the build stops at the next step with `CLUSTERING_CANCELLED`.

### User Story 3 - Analysts get one clustering everywhere (Priority: P2)

An analyst clusters vectors through the served data-science method, the SQL aggregate, UQL graph analytics or the Python kernel. Every surface runs the same kernel with the same refusals.

**Acceptance Scenarios**:

1. **Given** two separated blobs, **When** the analyst calls `DsKMeans` with `k = 2`, **Then** each blob gets one label, and a repeat call returns the same labels.
2. **Given** 2 rows, **When** the analyst calls `DsKMeans` with `k = 5`, **Then** the engine refuses with `CLUSTERING_K_EXCEEDS_ROWS`.
3. **Given** a row with a NaN, **When** any surface clusters it, **Then** it refuses with `CLUSTERING_INPUT_NOT_FINITE` and names the row.

### User Story 4 - Indexes stay fresh as data arrives (Priority: P2)

An ingest pipeline adds embeddings to an indexed graph all day. The index tracks how far the data has drifted from its centroids. It retrains in the background when the drift passes a threshold, warm-started from the tracked centroids.

**Acceptance Scenarios**:

1. **Given** an index trained on one distribution, **When** ingest adds vectors from a shifted distribution, **Then** the drift ratio rises and passes `cluster.refresh_drift_ratio`.
2. **Given** drift past the threshold, **When** the periodic sweep runs, **Then** one background retrain starts and serving continues on the old index until the swap.

### User Story 5 - Operators see and tune every training run (Priority: P3)

An operator watches a long build. Each run reports its state, step, budget, shift and elapsed time. Every tunable value is a named knob with a default, bounds and a unit.

**Acceptance Scenarios**:

1. **Given** a running build, **When** the operator reads the background-task progress, **Then** the run shows its state and its step out of the step budget.
2. **Given** a knob-set revision that moves `cluster.max_steps`, **When** the revision is checked, **Then** it is refused with `KNOB_NOT_TUNABLE`.

### Edge Cases

- `k = 0` refuses with `CLUSTERING_K_INVALID`.
- `k` above the row count refuses with `CLUSTERING_K_EXCEEDS_ROWS` unless the caller asks for at most `k`.
- Fewer than `k` distinct rows refuse with `CLUSTERING_TOO_FEW_DISTINCT` unless the caller asks for at most `k`.
- An empty input, a zero width or rows of unequal width refuse with `CLUSTERING_SHAPE_INVALID`.
- A NaN or infinite value refuses with `CLUSTERING_INPUT_NOT_FINITE` and names the first row and column.
- A centroid that receives no points for `cluster.empty_patience` steps reseeds to the farthest batch point; the lowest index wins a tie.
- A run that reaches `cluster.max_steps` ends `BudgetExhausted`, never loops.
- An input of at most one batch runs the exact full-batch mode.
- A cancel request stops the run at the next step boundary; no partial centroids are returned.
- A training memory need above `cluster.memory_budget_bytes` refuses before any allocation.
- A write during an index build lands in the build journal and is replayed at the swap.

## Requirements

- **FR-001**: One kernel module owns clustering, its types, refusals and knobs. Rows: `MEDULLA-CLUSTER-R001`.
- **FR-002**: The kernel trains with seeded mini-batch updates and moving-average centroids, sized by a scaling law. Rows: `MEDULLA-CLUSTER-R002`.
- **FR-003**: The same seed and input give bit-identical centroids at any thread count. Rows: `MEDULLA-CLUSTER-R003`.
- **FR-004**: Each run is an explicit state machine that reports progress and honours cancellation. Rows: `MEDULLA-CLUSTER-R004`.
- **FR-005**: Each run is bounded and never spins. Rows: `MEDULLA-CLUSTER-R005`.
- **FR-006**: Training memory follows a stated formula and a budget. Rows: `MEDULLA-CLUSTER-R006`.
- **FR-007**: The default path runs on the baseline instruction set in native Rust. Rows: `MEDULLA-CLUSTER-R007`.
- **FR-008**: Every current caller runs on the kernel and every old copy is deleted. Rows: `MEDULLA-CLUSTER-R008`.
- **FR-009**: Indexes track centroid drift online and retrain in the background past a threshold. Rows: `MEDULLA-CLUSTER-R009`.
- **FR-010**: A benchmark proves build time, recall, inertia and memory against the full-batch mode. Rows: `MEDULLA-CLUSTER-R010`.
- **FR-011**: An index build never blocks readiness, writers or request workers. Rows: `MEDULLA-CLUSTER-R011`.

## Success Criteria

- **SC-001**: Recall@10 on the recall bench is within 0.5 points of the full-batch mode at 100,000 vectors. Rows: `MEDULLA-CLUSTER-R010`.
- **SC-002**: Training is at least 3 times faster than the full-batch mode at 100,000 or more vectors. Rows: `MEDULLA-CLUSTER-R010`.
- **SC-003**: Centroids are bit-identical across 1, 2 and 8 threads. Rows: `MEDULLA-CLUSTER-R003.1`.
- **SC-004**: A writer waits at most 50 ms for the embedding store during a build. Rows: `MEDULLA-CLUSTER-R011.2`.
- **SC-005**: Peak training memory stays within the R006 formula plus 64 KiB. Rows: `MEDULLA-CLUSTER-R006`.
- **SC-006**: Inertia of the mini-batch mode is at most 1.03 times the full-batch inertia on the bench data. Rows: `MEDULLA-CLUSTER-R010`.

## Assumptions

- `MEDULLA-DECISION-ENGINE-R179` keeps the `KnobSpec` table form in `crates/medulla-decision-types/src/decision/knobs.rs`.
- The background-task progress registry is specified in parallel by `MEDULLA-OBSERVE` in `specs/engine-observability/`. R004.3 binds its provider row by ID before R004.3 is implemented.
- The thresholds of SC-001, SC-002 and SC-006 are starting values. The R010 bench measurement on this platform replaces them.
- Serving-time distance scoring and its accelerator seam stay with the vector index owner. Only training moves.
- `MEDULLA-DECISION-ENGINE-R175.2` plans seeded k-means for residual codebooks. When it lands, it calls this kernel. This spec does not edit that row.

## Why a new spec

The search ran `git grep -n -i -E "k-means|kmeans|lloyd|centroid|coarse quantizer|IvfPq" -- specs`.
It read MEDULLA-FEDERATED-QUERY, MEDULLA-DECISION-ENGINE, MEDULLA-CQ-XSRC and MEDULLA-INCREMENTAL-VIEWS.

- MEDULLA-FEDERATED-QUERY-R607.2 owns spectral partitions. It says "k-means with the given seed" and owns no kernel.
- MEDULLA-FEDERATED-QUERY-R720.3 owns the `CLUSTER BY EMBEDDING` stage. It calls the mining mixture model.
- MEDULLA-FEDERATED-QUERY-R040.5 owns low-storage index artifacts. It owns no training algorithm.
- MEDULLA-DECISION-ENGINE-R175.2 plans residual codebooks with seeded k-means. It owns no kernel.
- MEDULLA-FEDERATED-QUERY-R040.2 owns topic centroids for fusion scoring, not their training.

No row owns the centroid training that six crates implement today. One spec keeps one owner for it. Each linked row stays with its owner and is cited by ID.

## Scope and interfaces

In scope: the kernel module, its scaling law, knobs, run state machine, determinism, memory bound, refusals, every training caller, online drift tracking and the safety of the background index build.

Interfaces: `medulla_numeric::cluster::{fit, seed_centers, nearest, sq_dist, lane_reduce, ClusterConfig, ClusterRun, CentroidTracker}`, `Method::DsKMeans`, the SQL aggregate `kmeans(vec_col, k)`, the Python kernel `kmeans`, `IvfPq::train` and `semantic_activation::activate_one`.

Non-goals: density, hierarchical, mixture and medoid clustering (they stay in `crates/medulla-mining`), serving-time distance backends, the progress registry itself (MEDULLA-OBSERVE) and the knob registry (MEDULLA-DECISION-ENGINE-R179).

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-CLUSTER-R001` | plan.md "Phase 1: Design" | T-001 |  |
| FR-002 | `MEDULLA-CLUSTER-R002` | plan.md "Phase 1: Design" | T-002 |  |
| FR-003 | `MEDULLA-CLUSTER-R003` | plan.md "Phase 1: Design" | T-003 |  |
| FR-004 | `MEDULLA-CLUSTER-R004` | plan.md "Phase 1: Design" | T-004 |  |
| FR-005 | `MEDULLA-CLUSTER-R005` | plan.md "Phase 1: Design" | T-005 |  |
| FR-006 | `MEDULLA-CLUSTER-R006` | plan.md "Phase 1: Design" | T-006 |  |
| FR-007 | `MEDULLA-CLUSTER-R007` | plan.md "Build contract" | T-007 |  |
| FR-008 | `MEDULLA-CLUSTER-R008` | plan.md "Production caller chain" | T-008 |  |
| FR-009 | `MEDULLA-CLUSTER-R009` | plan.md "Phase 1: Design" | T-009 |  |
| FR-010 | `MEDULLA-CLUSTER-R010` | plan.md "Phase 1: Design" | T-010 |  |
| FR-011 | `MEDULLA-CLUSTER-R011` | plan.md "Production caller chain" | T-011 |  |
