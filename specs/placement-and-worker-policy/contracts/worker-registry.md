# Seam: worker registry and admission

Provider: medulla MEDULLA-DURABLE-KERNEL-R075
Consumer: medulla MEDULLA-PLACEMENT-R002

## Wire shape

Served `RegisterWorker` carries `cores`, `memory_bytes` and `lanes_supported` into the `WorkerRecord` (owner MEDULLA-DURABLE-KERNEL-R074). Served `AdmitWorkerTask` answers `WorkerGrant{slots, reserved_lane, lease_seconds, retry_after_seconds, grant_id}` or a typed refusal; the engine picks the reserved lane from the lane floors. Autosize, lane floors, widths and the queue-depth limit are served settings in `SchedulerConfig` (owner MEDULLA-DURABLE-KERNEL-R076); the worker reads none of them.

## Refusal codes

`WORKER_CAPACITY_REFUSED` (naming the binding limit), `WORKER_UNKNOWN`, `WORKER_CAPACITY_ZERO`, `ADMIT_HELD_FOR_RESERVE` (a claim that starves the hot-spare reserve, unless a queued lane has no running worker), `INGEST_QUEUE_FULL`.

## Authoritative types

`WorkerRecord` and `WorkerGrant` in the durable-kernel types; the pure sizing function `granted_slots` in `crates/medulla-work-types/src/worker_registry/grants.rs`.
