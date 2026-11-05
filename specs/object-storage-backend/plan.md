# MEDULLA-OBJECT-STORAGE-BACKEND — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

One physical store layer with two backends. `crates/medulla-storage` already binds owner-store authority per write transaction (`MEDULLA-DURABLE-KERNEL-R021`) and keeps a layout lineage registry with typed upgrade refusal (`MEDULLA-DURABLE-KERNEL-R003`). The object-store backend is a new physical binding under that same authority: immutable content-addressed segments, one immutable manifest per version, and one mutable `HEAD` updated by a conditional put. Consensus, placement, resharding and idempotency stay above the store layer and key off engine versions.

```text
server bootstrap
  -> open_persistence               crates/medulla-server/src/server_startup.rs
  -> StorageBackendSpec::admit      crates/medulla-storage/src/backend_spec.rs
  -> ObjectStoreBackend             crates/medulla-persistence/src/object_store_backend.rs
  -> ObjectFetcher / ObjectArchive  crates/medulla-storage/src/object/
  -> object_store crate trait       S3-compatible root
```

No new consensus, no new writer authority and no parallel layout registry.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-storage`, `crates/medulla-persistence`, `crates/medulla-persistence-ops`, `crates/medulla-server`, and the `object_store` crate already used by `crates/medulla-server-blob`. No new third-party dependency.
- **Storage**: one object root per deployment, laid out as `<root>/<tenant>/<graph>/{HEAD,manifests,segments}` and `<root>/archive/`; the local redb store stays the cache and the default.
- **Testing**: Rust tests in `crates/medulla-storage/tests/` (L2) and `crates/medulla-persistence/src/object_store_backend/tests.rs` (L3); served Python tests under `tests/test_object_storage_*.py` (L4); a nightly bench (L6).
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Storage and persistence additions to the existing engine crates.
- **Performance Goals**: A graph copy runs within p50 400 ms and p99 1,500 ms per 100,000 elements; the open cost is guarded by the cost gate.
- **Constraints**: Commit before ack. A read never writes the root. A refusal precedes every effect. Memory under 128 MiB for a graph copy.
- **Scale/Scope**: 9 rows over 6 user stories, 7 `FR-` labels and 5 `SC-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. The object root holds the engine's own durable copy.
- **R-II. Every wait has a deadline**: pass. Object calls run under the commit path deadline; the open fetch is bounded by `object_store.open_parallel_fetches`.
- **R-III. Every row sets budgets and names a benchmark**: pass. `benches/graph_copy.rs` and `benches/object_store_cost.rs` carry the budgets.
- **R-IV. Crates are small, layered and cut over hard**: pass. Pure object logic sits in `medulla-storage`; the `PersistenceBackend` implementation that wraps `RedbBackend` sits in `medulla-persistence`.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-storage/src/backend_spec.rs             existing: StorageBackendSpec and admission
crates/medulla-storage/src/object.rs                   new: module root
crates/medulla-storage/src/object/                     new: manifest.rs, fetch.rs, archive.rs, counting.rs
crates/medulla-persistence/src/object_store_backend.rs new: ObjectStoreBackend
crates/medulla-persistence-ops/src/store_upgrade/graph_copy.rs  new: copy_graph
crates/medulla-persistence/src/redb_backend/backend_shard_routes.rs  existing: paged export beside the snapshot export
src/operator_command.rs                                existing: graph-copy verb
benches/object_store_cost.rs, benches/graph_copy.rs    new: nightly benches
tests/test_object_storage_*.py                         new: served tests
```

## Phase 0: Research

No unknowns that block planning; no research.md.

## Phase 1: Design

1. **Compare-and-set manifest publish, not a second consensus layer.** Object-store conditional writes give the atomic publish. Raft stays the authority for anything cross-node.
2. **Bounded re-fetch for a cold open.** A cold node fetches the current version's closure and skips full history; history stays reachable by version.
3. **One S3 client.** Object access reuses the `object_store` trait the blob and cold-tier code already use.
4. **Layering.** Pure object logic in `medulla-storage` serves both the backend and the recovery archiver; the backend that wraps `RedbBackend` lives at the persistence level.
5. **The cost gate is a release gate.** One bounded regression guard covers the fixed open cost; it never runs in a pull request.

Data entities are in [data-model.md](data-model.md); the shortest served proof is in [quickstart.md](quickstart.md).

## Phase 2: Task approach

`tasks.md` holds one task per row, producer first. The backend spec precedes the read path; the commit path precedes the copy and shared-root rows. The archive (T007) runs in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- The no-duplicate-authority check confirms the object-store binding shares the store-authority chokepoint and never bypasses it.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Release proof follows `MEDULLA-CONTRACT`.

## Production caller chain

- `Shard::open` (`crates/medulla-store/src/redb_store/shard.rs:226`) to `admit_backend_setting` (`shard.rs:1062`) to `StorageBackendSpec::admit` (`crates/medulla-storage/src/backend_spec.rs:117`).
- `open_persistence` (`crates/medulla-server/src/server_startup.rs:419`) to `ObjectStoreBackend::open` to `ObjectFetcher::materialize`.
- `commit_mutation_body` (`crates/medulla-server-mutation/src/mutation/commit_entry.rs:327`) to `PersistenceBackend::commit_mutation_batch` (`crates/medulla-server-api/src/api/persistence.rs:275`) to `ObjectStoreBackend::commit_mutation_batch`.
- `OperatorCommand::run` (`src/operator_command.rs:64`) to `copy_graph`.
- Obsolete paths to delete: the `Unsupported` arm of `StorageBackendSpec::admit` for `object_store`, once R002 lands.

## Cross-repo seams

none
