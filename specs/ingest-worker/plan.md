# MEDULLA-INGEST-WORKER — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine owns the ingest queue, its policy and every admission rule. The runner process belongs to the connector SDK, and arbora hosts it. Two served paths carry the work. The queue path (`IngestEnqueue`, `IngestQueueDepth`, `TaskQueueClaim`, `TaskQueueAck`) enters at `crates/medulla-server-router` and runs in `crates/medulla-h-ingest/src/ingest_queue.rs`. The batch path (`SourceIngest`) enters at `crates/medulla-server-graph` and runs in `crates/medulla-h-ingest/src/source_ingestion.rs`. Each batch kind has one admission function in `crates/medulla-h-ingest/src/source_ingestion/`.

```text
runner (SDK) --> register_worker > admit_worker_task > TaskQueueClaim > TaskQueueRenew > TaskQueueAck
producer     --> IngestEnqueue > validate > lane + partition > TaskQueueEnqueue
connector    --> SourceIngest > prepare > resolve_mappings > match batch.kind > one envelope > finish
```

| Concern | Owning code | Rows |
|---|---|---|
| Job types, lanes, retry | `crates/medulla-source-types/src/ingest_job.rs` | `MEDULLA-INGEST-WORKER-R001.1` to `MEDULLA-INGEST-WORKER-R001.8` |
| Queue handler | `crates/medulla-h-ingest/src/ingest_queue.rs` | `MEDULLA-INGEST-WORKER-R001.4`, `MEDULLA-INGEST-WORKER-R001.9` to `MEDULLA-INGEST-WORKER-R001.12` |
| Metrics | `crates/medulla-kernel/src/metrics/ingest.rs` | `MEDULLA-INGEST-WORKER-R001.13`, `MEDULLA-INGEST-WORKER-R604.5` |
| Drains | `crates/medulla-h-ingest/src/ingest_drain.rs` | `MEDULLA-INGEST-WORKER-R605.2` |
| Documents and transcripts | `crates/medulla-h-ingest/src/source_ingestion/documents.rs`, `transcript.rs` | `MEDULLA-INGEST-WORKER-R003.8`, `MEDULLA-INGEST-WORKER-R003.14` |
| Readers and dedup | `crates/medulla-document/src/fragment.rs`, `crates/medulla-compute/src/sketch.rs` | `MEDULLA-INGEST-WORKER-R700.1`, `MEDULLA-INGEST-WORKER-R702.1` |
| Governance, permissions, sweep, containers | `crates/medulla-h-ingest/src/source_ingestion/` | `MEDULLA-INGEST-WORKER-R900.2`, `MEDULLA-INGEST-WORKER-R902.2`, `MEDULLA-INGEST-WORKER-R903.2`, `MEDULLA-INGEST-WORKER-R904.3` |

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: The workspace crates named in Project Structure. Reader crates (`lopdf`, `zip`, `quick-xml`, `calamine`, `mail-parser`, `csv`) enter only after `tests/test_third_party_source_policy.py` passes, each behind a feature `reader-<format>`.
- **Storage**: The durable job store behind `MEDULLA-DURABLE-KERNEL-R072` holds tasks. Graph nodes and edges hold atoms, proposals, containers and evidence. No second store exists.
- **Testing**: Rust unit and handler tests per crate, served Python tests under `tests/ingest/`, nightly benches.
- **Target Platform**: Linux x86_64 and aarch64 hosts without AVX2 assumptions.
- **Project Type**: Library crates and one handler crate.
- **Performance Goals**: Claim with admission p99 under 1 ms at 64 workers. Materialization p99 under 300 ms per 1,024-record batch. Each row states its own budget.
- **Constraints**: Handler crates never call a sibling handler; a call across handlers goes through a seam in `crates/medulla-server-ops/src/api/ingest.rs`. Every refusal has one stable code and changes no state.
- **Scale/Scope**: 116 leaf rows in five user stories and 13 `FR-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate and the engine store.
- **R-II. Every wait has a deadline**: pass. Leases, grace windows and sweep intervals are named values.
- **R-III. Every row sets budgets and names a benchmark**: pass. Hot-path rows name a nightly bench group of `MEDULLA-DECISION-ENGINE-R178.1`.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates, and no handler calls a sibling handler.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-source-types/src/ingest_job.rs        existing: envelope, lanes, partition key, retry
crates/medulla-source-types/src/source_ingestion.rs  existing: batch, receipt, batch kind (extended)
crates/medulla-source-types/src/source_document.rs   existing: normalized document, quarantine
crates/medulla-source-types/src/source_transcript.rs existing: transcript thread and message
crates/medulla-types/src/                            existing: knowledge_atom, ontology_proposal, extracted_fact, ingest_rules, source_governance; new: ingest_lanes, ingest_drain, ingest_materialize, data_prep_declaration
crates/medulla-h-ingest/src/                         existing: source_ingestion.rs; new: ingest_queue, ingest_drain, atom_context, data_prep_declaration, source_ingestion/*
crates/medulla-document/src/                         existing: processing, transcript, html; new: atoms, fragment, readers
crates/medulla-compute/src/sketch.rs                 existing: MinHashSketch (LshIndex added)
crates/medulla-core/src/                             new: source_hierarchy
crates/medulla-kernel/src/metrics/                   existing: outbox; new: ingest
crates/medulla-server-router/src/dispatch/router/source_ingest.rs  existing: dispatch arms
crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs  existing: SourceIngest route
crates/medulla-server-ops/src/api/ingest.rs          existing: IngestApi and JobsApi seams
tests/ingest/                                        new: served Python tests
benches/                                             new: root benches named by the rows
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns; no research.md.

## Phase 1: Design

The rules below bind every row.

1. **The engine owns state and policy.** Queue, leases, retry, dead letter, depth and metrics are engine behaviour. Job bodies parse files, call models and touch sources with SDK credentials, so the engine never runs a source effect or holds a source secret.
2. **One admission path.** Every connector batch converges on `prepare` and `admit_records`. The batch kind selects the admission function. Authorization and the committed-replay shortcut come first; mappings resolve once; validators share that snapshot.
3. **One envelope per batch.** A batch commits one accepted or quarantine envelope. A normalization failure returns a prepared quarantine outcome and never bypasses the commit. A quarantine stores restricted evidence only and never advances a checkpoint.
4. **No second policy writer.** The ack handler is the only dead-letter writer. The claim handler evaluates admission itself. One routing function and one gauge set exist.
5. **Cross-handler calls use seams.** The queue handler reaches the task queue through `jobs_seam`, and the task-queue handler reaches `on_ingest_ack` and `on_ingest_claim` through `ingest_seam`. The implementations are installed in `crates/medulla-server/src/seams_install.rs`.
6. **Wire changes carry no shim.** A changed Rust wire type updates the generated Python models in `medulla/generated/` and gets a Rust round-trip vector in the same change.
7. **Names and ids.** Ids are framed digests built with `Digest256::framed` under a domain tag per kind. No component name carries a version suffix.
8. **Knobs.** Each tunable value is a named knob with a default, bounds and a unit; security-relevant values are `Fixed`.

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row in table order, producer first. A model row precedes the rows that wire it. A row lands in one slice with its caller, so no type lands without a production caller. Rows that edit `source_ingestion.rs` (`prepare`, `admit_record`) run in series; rows in separate new files run in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every hot-path row names a nightly bench group and its budget, per article R-III; a bench is a service level and never gates a pull request.
- A public item in an ingest crate has a non-test caller under a handler or server crate. `MEDULLA-CONTRACT-R055` gates this.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- A handler crate never calls a sibling handler crate; a call across handlers uses a seam.

## Production caller chain

- `Method::IngestEnqueue` arm (`crates/medulla-server-router/src/dispatch/router/source_ingest.rs::dispatch_source_ingest_methods`, new) to `enqueue_ingest_job` (`crates/medulla-h-ingest/src/ingest_queue.rs`, new) to `jobs_seam` (`crates/medulla-server-ops/src/api/ingest.rs`) to the task-queue handler of `MEDULLA-DURABLE-KERNEL-R072`.
- `Method::TaskQueueAck` and `TaskQueueClaim` arms of `MEDULLA-DURABLE-KERNEL-R072` to `ingest_seam` to `on_ingest_ack` and `on_ingest_claim` in `ingest_queue.rs` (new).
- `Method::SourceIngest` (`crates/medulla-protocol-types/src/protocol/method/method_10.rs`) to `route_source_ingestion_or_resources` (`crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs`) to `route_source_ingestion_dispatch` to `route_source_ingestion_request` to `prepare` (`crates/medulla-h-ingest/src/source_ingestion.rs`) to the admission function for the batch kind, then `apply_one_change_envelope` (`crates/medulla-server-graph/src/dispatch/change_envelope.rs`) and `finish`.
- `Method::AtomContext`, `Method::SourceDrainStatus`, `Method::DataPrepDeclare` and `Method::IngestQueueDepth` arms of `source_ingest.rs` (new) to `atom_context`, `drain_status`, `declare_data_prep` and `queue_depth` in `crates/medulla-h-ingest/src/`.
- Background: `src/main.rs` starts `spawn_reconcile_sweep` (`crates/medulla-h-ingest/src/source_ingestion/sweep.rs`, new) and registers the metrics collector.

## Cross-repo seams

- Ingest worker runner: `MEDULLA-INGEST-WORKER-R001.14` provides, `SCION-SOURCE-INGEST-R010` consumes. Contract: [scion-ingest-runner.md](contracts/scion-ingest-runner.md).
- Source batch kinds from connectors: `MEDULLA-INGEST-WORKER-R902.3.1` provides, `SCION-SOURCE-INGEST-R805.1` consumes. Contract: [scion-source-batch-kinds.md](contracts/scion-source-batch-kinds.md).
- Data-prep declaration: `MEDULLA-INGEST-WORKER-R001.1008` provides, `ARBORA-FLEET-R058` consumes. Contract: [arbora-data-prep-declaration.md](contracts/arbora-data-prep-declaration.md).
- Document and transcript batches: `MEDULLA-INGEST-WORKER-R003.8` provides, `SCION-SOURCE-INGEST-R010` consumes. Contract: [scion-document-transcript-ingest.md](contracts/scion-document-transcript-ingest.md).
