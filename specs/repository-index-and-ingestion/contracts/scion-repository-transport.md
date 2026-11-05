# Seam: the connector transport sends repository batches and halves oversized ones

Provider: medulla MEDULLA-REPO-INGEST-R008
Consumer: scion-sdk SCION-REPOSITORY-TRANSPORT-R004

The connector SDK reads a repository snapshot from a provider and sends it to the engine. The SDK never writes the graph itself. It sends `Method::IndexRepository` batches and resends a smaller batch when the engine refuses a large one.

## Wire shape

The request is `Method::IndexRepository { files_msgpack, scope }`. The scope holds the repository id, immutable commit ids, ref status, file-version memberships and tombstones. The result is an `IndexResult` with one ordered outcome per submitted unique blob. For governed observations the SDK sends `Method::SourceIngest` with the exact expected checkpoint and a mode of `full`, `delta` or `reconcile` (`SCION-SOURCE-INGEST-R004.13` to `R004.18`).

## Refusal codes

`REPOSITORY_BATCH_TOO_LARGE` names the observed bytes and items. It commits nothing, so the sender halves the batch and resends it without changing identity or replay semantics. `INDEX_BASE_MISMATCH` refuses a delta whose base revision does not match. Both codes register in `crates/medulla-types/src/contract/server_error_code.rs`.

## Owner rows

- medulla: MEDULLA-REPO-INGEST-R008 (oversized batches return a stable retry code) and MEDULLA-REPO-INGEST-R006.4 (bounded incremental indexing).
- scion-sdk: SCION-REPOSITORY-TRANSPORT-R004 (send and halve) and SCION-SOURCE-INGEST-R004.13 (delta transport).
