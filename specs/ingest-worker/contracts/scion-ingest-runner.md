# Seam: ingest worker runner contract

Provider: medulla MEDULLA-INGEST-WORKER-R001.14
Consumer: scion-sdk SCION-SOURCE-INGEST-R010

## Wire shape

The runner registers with `register_worker`, receives a grant from `admit_worker_task`, claims with `task_queue_claim`, renews with `task_queue_renew` every third of `lease_ms` and acknowledges with `task_queue_ack` after execution. A claim returns one `IngestJobEnvelope` and `lease_ms`. A body killed before the ack sends no ack, and the job redelivers after the lease lapses.

## Refusal codes

`ADMIT_HELD_FOR_RESERVE` is a signal to back off, never to try another lane. `TASK_LEASE_EXPIRED` refuses a renew or ack on an expired or foreign lease. `INGEST_QUEUE_FULL` refuses a producer above the depth limit.

## Authoritative types

`IngestJobEnvelope` in `crates/medulla-source-types/src/ingest_job.rs`. The engine holds no source credential.

## Test vectors

`tests/ingest/test_ingest_worker_contract_served.py::test_runner_call_sequence` is the vector; the runner proves its side against the same request and reply types.
