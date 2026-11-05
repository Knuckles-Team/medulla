# Seam: task queue and worker registry for the ingest runner

Provider: medulla MEDULLA-DURABLE-KERNEL-R072
Consumer: scion-sdk SCION-SOURCE-INGEST-R011

## Wire shape

The runner calls the served methods `TaskQueueEnqueue`, `TaskQueueClaim` and `TaskQueueAck`, and the worker methods `WorkerRegister`, `WorkerHeartbeat` and `AdmitWorkerTask` (`MEDULLA-DURABLE-KERNEL-R074.2`, `MEDULLA-DURABLE-KERNEL-R075`). Enqueue carries `queue`, `kind`, `payload_ref`, `idempotency_key`, `priority` and `not_before_ms`. Claim carries `worker_id`, `grant_id` and `lease_ms`. Ack carries `task_id`, `worker_id`, an outcome of `Succeeded`, `Failed` or `Retry`, and a short detail. A claim names no lane or partition. The caller supplies no tenant; the engine takes tenant and principal from the verified request context. An unacked claim redelivers after lease expiry, also after a restart.

## Refusal codes

`TASK_QUEUE_NAME_BLANK`, `TASK_LEASE_ZERO`, `TASK_ACK_NOT_CLAIMED`, `TASK_ACK_LEASE_EXPIRED`, `TASK_DUPLICATE`, `WORKER_UNKNOWN` and `ACCESS_DENIED`. They are registered in `ServerErrorCode`. The scopes are `task:enqueue`, `task:work` and `worker:register`.

## Authoritative types

`crates/medulla-types/src/task_queue.rs` (`MEDULLA-DURABLE-KERNEL-R071`) and `crates/medulla-work-types/src/worker_registry.rs` (`MEDULLA-DURABLE-KERNEL-R074.1`). The Python client methods are in `medulla/client.py` (`MEDULLA-DURABLE-KERNEL-R073`).

## Test vectors

`tests/task_queue_served.rs` and `tests/worker_registry_served.rs` hold the served vectors. The runner tests in the scion-sdk repository replay the same enqueue, claim and ack sequences against a served engine.
