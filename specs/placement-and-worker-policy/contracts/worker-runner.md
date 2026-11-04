# Seam: worker runner obeys the grant

Provider: medulla MEDULLA-PLACEMENT-R002
Consumer: scion-sdk SCION-SOURCE-INGEST-R011.1

## Wire shape

The SDK runner's call sequence: `register_worker`, `admit_worker_task`, `task_queue_claim`, `task_queue_renew`, then `task_queue_ack`. The runner claims only after registering, uses the granted slot count and lane, and treats an admission refusal as a back-off signal, never a signal to try another lane. Claim returns one job envelope and a lease; a body killed before ack redelivers after the lease lapses.

## Refusal codes

`ADMIT_HELD_FOR_RESERVE`, `TASK_LEASE_EXPIRED`. The engine holds no source credential; bodies read sources with SDK credentials.
