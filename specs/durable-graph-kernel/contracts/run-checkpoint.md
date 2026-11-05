# Seam: run checkpoint with memory references

Provider: medulla MEDULLA-DURABLE-KERNEL-R028.1
Consumer: arbora ARBORA-A2A-R001.2

## Wire shape

The caller sends `PublishRunCheckpoint` with `CheckpointContent`: the run id, the work item id, the lease epoch and fencing token, the checkpoint revision and two lists of `{document_id, revision, digest}` for short-term and long-term memory. The checkpoint pointer and its memory snapshot commit in one MutationBatch. Tenant and graph come only from the verified request context. The engine performs no resume and no stream operation; the harness owns the resume request and the host owns the stream cursor.

## Refusal codes

`CHECKPOINT_STALE_REVISION`, `CHECKPOINT_FENCE_STALE`, `CHECKPOINT_MEMORY_REF_MISSING`, `CHECKPOINT_DIGEST_MISMATCH` and `ACCESS_DENIED`, all in `ServerErrorCode`. A refused publish leaves the checkpoint head unchanged.

## Authoritative types

`CheckpointMemoryRef`, `CheckpointContent` and `PublishRunCheckpointRequest` in `crates/medulla-work-types/src/run_checkpoint.rs`. The publish code is `crates/medulla-work-store/src/redb_store/work_item/checkpoint.rs`.

## Test vectors

`tests/adopt_workitem_authenticated_replay.rs` holds the served vectors for publish, replay, owner isolation, stale fence and missing memory reference.
