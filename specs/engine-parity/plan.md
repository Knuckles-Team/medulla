# MEDULLA-ENGINE-PARITY — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine stays one library. That choice keeps five properties a second engine breaks: one process with one commit path and one backup, deterministic replay from the committed log, sealed create-only records, epistemic and ontology primitives in the same transaction as the data, and the same library on a small board and in the cluster. The cost is maturity work: planner tuning, index kinds, constraint semantics, tool compatibility and operations practice. This plan builds that parity piece by piece, only for named consumers, and states each gap in the compatibility matrix (R009) instead of hiding it.

```text
outside SQL tool
  -> pgwire listener             crates/medulla-wire/src/pgwire/mod.rs
  -> classify                    crates/medulla-sql-lang/src/sql/classify.rs
  -> sql_catalog / exec_sql      crates/medulla-h-query, crates/medulla-sql
  -> TableStore::commit_txn_batch  crates/medulla-table-store/src/tables/store.rs
  -> RedbBackend writer thread   crates/medulla-persistence/src/redb_backend/
  -> CdcHub / archive            crates/medulla-server-cdc, crates/medulla-storage
```

The named consumer of each rollup (the demand gate below; `crates/medulla-types/tests/engine_capabilities.rs` reads these rows):

```text
| Rollups | Named consumer |
| R001 to R005 | Concurrent agent writes through arbora; MEDULLA-SCM changeset acceptance |
| R006 to R009 | Outside BI and SQL tools for the catalog and governance rows; the native-hosting pilot of MEDULLA-UNIFIED-DATA-PLANE-R026 |
| R010 to R014 | Native-hosted application tables of MEDULLA-UNIFIED-DATA-PLANE-R024 |
| R015 to R017 | Change subscribers of MEDULLA-UNIFIED-DATA-PLANE-R009; operator recovery; read scale |
| R018 | Catalog profiling and quality jobs of MEDULLA-UNIFIED-DATA-PLANE-R761 and R762 |
| R019 | Every attached-source adapter |
| R020 | The benchmark gate of MEDULLA-DURABLE-KERNEL-R042 |
```

Every arrow stays inside the library except the wire listener, the archive target and the attached sources. No arrow writes engine data to an outside database.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-persistence`, `crates/medulla-transaction`, `crates/medulla-table-store`, `crates/medulla-tables`, `crates/medulla-sql`, `crates/medulla-sql-lang`, `crates/medulla-wire`, `crates/medulla-wire-core`, `crates/medulla-server-cdc`, `crates/medulla-storage`, `crates/medulla-statechart`. No new third-party dependency.
- **Storage**: redb store files with new catalog tables `sql_sequences` and `sql_matviews`; archive segments in the object root of `MEDULLA-OBJECT-STORAGE-BACKEND`.
- **Testing**: Rust tests in `crates/medulla-sql-tests/tests/`, `crates/medulla-transaction/tests/` and root `tests/pgwire_roundtrip.rs` (L1 to L3); served Python tests under `tests/` (L4); nightly benchmark groups in `benches/engine_parity.rs` (L6).
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Storage, query and wire additions to the existing engine crates.
- **Performance Goals**: At least 20,000 commits per second for 64 writers; single-row commit p99 at most 5 ms; prepared point read p99 at most 1 ms; targets W1 to W5 of `MEDULLA-ENGINE-PARITY-R005.1`.
- **Constraints**: One committer per store file. A refusal precedes any write. Conflicts are typed. No secret is stored. Benchmarks never gate a pull request.
- **Scale/Scope**: 51 leaf rows over 5 user stories, 8 `FR-` labels and 6 `SC-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Outside databases are attached sources only (R019).
- **R-II. Every wait has a deadline**: pass. The enqueue wait is bounded by `MEDULLA_REDB_ENQUEUE_TIMEOUT_MS` (R001.1) and retries are bounded by `tables.occ_max_retries` (R004.2).
- **R-III. Every row sets budgets and names a benchmark**: pass. Each hot-path row names a budget and a nightly bench group.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; new table features sit in `medulla-table-store`.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-persistence/src/redb_backend/   existing: group commit config, shard writer, send_with_deadline
crates/medulla-transaction/src/occ.rs          existing: TxnFootprint, disjoint
crates/medulla-table-store/src/tables/         existing store.rs; new trigger.rs, sequence.rs, matview.rs
crates/medulla-tables/src/                     existing: schema.rs, index.rs extended
crates/medulla-sql-lang/src/sql/classify.rs    existing: new statement kinds
crates/medulla-sql/src/sql/catalog.rs          existing: new catalog providers
crates/medulla-wire-core/src/wire/sqlstate.rs  new: closed SQLSTATE table
crates/medulla-identity-types/src/identity/    existing ops.rs; new wire_login.rs
crates/medulla-server-cdc/src/cdc.rs           existing: emit_row_changes
crates/medulla-storage/src/recovery/archive.rs new: archive_scope, replay_to
benches/engine_parity.rs                       new: W1 to W5 and the nightly groups
```

## Phase 0: Research

No unknowns that block planning; no research.md.

## Phase 1: Design

1. **One library, one committer per store file.** Group commit folds queued commands into one write transaction and one fsync; this plan adds only a bounded enqueue wait.
2. **Native writes keep their version check.** Graph mutations keep `MEDULLA-DURABLE-KERNEL-R084`; footprints (R002) cover user-table transactions only.
3. **One compatibility matrix.** `PgCompatibilityMatrix` is the single matrix; the classifier consults it and a refused form answers `0A000`.
4. **One SQLSTATE table.** Every wire refusal maps through `sqlstate_for`; an unknown code keeps `58000`.
5. **No second change feed.** Row capture feeds the `ReadGraphChanges` feed of `MEDULLA-DURABLE-KERNEL-R043.100`; there is no new subscribe method.
6. **Demand gate.** A rollup enters the work queue only with a named consumer; the consumers are `MEDULLA-SCM-R003` (R004), `ARBORA-OPS-R858` (R007) and `MEDULLA-DURABLE-KERNEL-R035` (R009, R015).

Data entities are in [data-model.md](data-model.md); the shortest served proof is in [quickstart.md](quickstart.md); each seam has a document in `contracts/`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, producer first within each user story. Tasks that edit `crates/medulla-table-store/src/tables/store.rs` run in series; hot-path proofs run in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- The no-duplicate-authority check confirms one committer per store file and one change feed.
- Each nightly benchmark gates a 10 percent regression against `benches/baselines/engine_parity.json`.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- Served `AddNode` to `commit_mutation_body` (`crates/medulla-server-mutation/src/mutation/commit_entry.rs:327`) to `record_durable` (`crates/medulla-persistence/src/redb_backend/trait_mutations.rs:5`) to the shard writer (`shard_writer.rs:474`).
- `COMMIT` over the SQL wire to `sql_catalog.rs:269` to `commit_txn_batch` (`crates/medulla-table-store/src/tables/store.rs:2022`) to `commit_txn_batch_inner` (:2095) to `TxnFootprint::validate` (`crates/medulla-transaction/src/occ.rs:92`).
- Pgwire startup to `on_startup` (`crates/medulla-wire/src/pgwire/auth.rs:176`) to `resolve_wire_login`; a statement to `do_query` (`crates/medulla-wire/src/pgwire/mod.rs:925`) to `classify` (`crates/medulla-sql-lang/src/sql/classify.rs:520`).
- `medulla-server restore` to `src/offline_tools/restore.rs` to `restore_bundle` (`crates/medulla-persistence/src/backup.rs:801`) to `replay_to`.
- Obsolete paths to delete: the per-column serial counter of `store.rs`, read once into the sequence `high_mark` and then removed (R013.2).

## Cross-repo seams

- Wire login grant and revoke: `MEDULLA-ENGINE-PARITY-R007.4` provides, `ARBORA-OPS-R858.1` consumes. Contract: [arbora-wire-logins.md](contracts/arbora-wire-logins.md).
- Engine capability report: `MEDULLA-ENGINE-PARITY-R020.2` provides, `ARBORA-OPS-R006` consumes. Contract: [arbora-engine-capabilities.md](contracts/arbora-engine-capabilities.md).

## Decisions (see research.md)

- Sequences are persisted in the existing `__sql_seq__` allocator table under NUL-prefixed keys; no new owner table.
- One SQLSTATE table in `crates/medulla-wire-core/src/wire/sqlstate.rs`; both `user_err` functions read it.
- `COPY ... TO STDOUT` is a plain read through the authorized read path.
- Partial and expression indexes are used only by a planner that proves the predicate (R010.2).
- `R005.1` depends on `R002.2` (its conflict-heavy workload); `R001.3` lands with `R005.1`.
- `R015.1`: the hub is installed as the tenant store's row-change sink by `handle_sql`; the ring is the knob `cdc.ring_events`; a row event is a `Row` change on `ReadGraphChanges` gated by `cdc:read` and the table's SELECT grant.
