# MEDULLA-DURABLE-KERNEL — Data model

The types below are the durable entities that rows of this spec define or extend. Each lives in the crate shown and has one owner.

## Entities

- **MutationBatch** (`crates/medulla-types`): the sole persistent write envelope. Fields: members, idempotency key, expected versions, actor. One admission decision and one terminal outcome per batch.
- **KvBlockKey** (`crates/medulla-kvcache/src/version.rs`): model id, model revision and token-prefix hash. Equal prefixes give equal keys; a different model revision gives a different key (`MEDULLA-DURABLE-KERNEL-R083.1`).
- **TaskEnqueue, TaskClaim, TaskAck** (`crates/medulla-types/src/task_queue.rs`): the typed durable task-queue model. Bounds: queue name 1 to 128 characters of `[a-z0-9._-]`, payload reference at most 512 bytes, priority 0 to 9, lease 1 000 to 300 000 ms (`MEDULLA-DURABLE-KERNEL-R071`).
- **WorkerRegistration, WorkerRecord** (`crates/medulla-work-types/src/worker_registry.rs`): capacity-bearing worker facts with deterministic register, heartbeat and expiry transitions (`MEDULLA-DURABLE-KERNEL-R074.1`).
- **SchedulerConfig** (`crates/medulla-types/src/scheduler_config.rs`): total workers, queue depth, per-lane floors and the worker time-to-live (`MEDULLA-DURABLE-KERNEL-R076.1`).
- **GraphSnapshot, GraphChangeEvent** (`crates/medulla-types/src/graph_change_feed.rs`): the snapshot with its high-water cursor and the ordered change event with its commit-to-visible timing (`MEDULLA-DURABLE-KERNEL-R043.100.1`).
- **CheckpointContent, CheckpointMemoryRef** (`crates/medulla-work-types/src/run_checkpoint.rs`): the run checkpoint and its memory references (`MEDULLA-DURABLE-KERNEL-R028.1`).
- **RequiredTestModule** (`crates/medulla-types`): a typed category and validated declaration of the test modules a gate must run (`MEDULLA-DURABLE-KERNEL-R011.1`).

## Relationships

A MutationBatch commits graph, table, KV and time-series members together or none. An outbox row follows a committed batch and keeps its delivery state on the owning node. A TaskEnqueue becomes a job of the job store, and a TaskClaim leases it to a registered worker. A GraphChangeEvent follows a committed change envelope and never precedes it.
