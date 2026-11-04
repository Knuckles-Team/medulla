# Seam: ingest-worker row wording

Provider: medulla MEDULLA-INGEST-WORKER-R001
Consumer: medulla MEDULLA-PLACEMENT-R002

## Wire shape

Text-level seam: the ingest-worker rows state that the engine, not the worker, calls `lane_for`; the claim order is derived by the engine from the partition key; worker counts, floors and limits are read from served settings; the grant names the reserved lane; the queue depth limit is `SchedulerConfig.max_queue_depth`; the runner claims after registering and backs off on refusal; the `--workers` flag is an optional request ceiling. The placement rows MEDULLA-PLACEMENT-R002.6 to MEDULLA-PLACEMENT-R002.12 and MEDULLA-PLACEMENT-R002.15 are retired into this wording.

## Refusal codes

None new on this seam: `INGEST_QUEUE_FULL` (depth at or above the served limit) is the only ingest-side refusal named.
