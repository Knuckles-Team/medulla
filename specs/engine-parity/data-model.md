# MEDULLA-ENGINE-PARITY — Data model

This file describes the entities that the rows add or extend.

## Entities

### TxnFootprint, KeyRange and CommittedWrites

- Fields: `TxnFootprint{snapshot_seq, reads, writes}`; `KeyRange` is a table and a key range; `CommittedWrites` is the writes of one commit.
- Owner: `crates/medulla-transaction/src/occ.rs`. `TableStore` keeps a ring of the last `tables.occ_history_commits` commits.

### SqlFormEntry and SqlFormState

- Fields: `SqlFormEntry{name, kind, state}`; `kind` is `Statement`, `Function`, `Operator` or `Type`; `SqlFormState` is `Supported`, `Partial` or `Refused`.
- Owner: `crates/medulla-mutation-types/src/pg_operational_surface.rs`, inside `PgCompatibilityMatrix`. The matrix is closed: an absent form is `Refused`.

### SQLSTATE table

- One closed table from stable engine codes to SQLSTATE values, in new `crates/medulla-wire-core/src/wire/sqlstate.rs`. An unknown code keeps `58000`.

### WireLoginRecord and WireLoginKind

- Fields: `WireLoginRecord{login, kind, verifier, principal, tenant, created_ms, revoked_ms, scopes}`; `WireLoginKind` is `Password`, `Certificate` or `Token`.
- Owner: new `crates/medulla-identity-core-types/src/identity/wire_login.rs`. No secret is stored; the verifier is a stored key or a digest.

### SecondaryIndexSpec extension

- Adds `predicate: Option<CheckExpr>` and, per column, `expr: Option<IndexExpr>` (`Lower`, `Upper`, `Abs`, `Length`). Owner: `crates/medulla-tables/src/index.rs`. The schema version moves to 2.

### TriggerSpec and TriggerReaction

- Fields: `TriggerSpec{table, name, timing, events, machine_id}`; `TriggerReaction{event_id, trigger, statement_id}` in an append-only ledger.
- Owner: new `crates/medulla-table-store/src/tables/trigger.rs`.

### Sequence

- Fields: `Sequence{name, start, increment, cache}`; stored in the table `sql_sequences` as `{high_mark, last_issued}`. Owner: new `crates/medulla-table-store/src/tables/sequence.rs`.

### MatViewSpec

- Fields: `MatViewSpec{name, select_sql, inputs, owner, refreshed_seq, stale}`; stored msgpack in the redb catalog table `__sql_matviews__`, written only by `TxnOp::PutMatView` and removed by `TxnOp::DropMatView`, with the result rows in a user table named like the view (R014.1). Owner: new `crates/medulla-table-store/src/tables/matview.rs`.

### CdcEvent extension

- Adds the kinds `InsertRow`, `UpdateRow`, `DeleteRow` to `CdcKind` and a `table` field to `CdcEvent`. Owner: `crates/medulla-query-types/src/wire_streaming.rs`.

### ArchivePolicy and ArchiveSegment

- Fields: `ArchivePolicy{segment_bytes, retain_s, interval_s}`; `ArchiveSegment{scope_key, records}` (each record the stored ledger bytes); `ArchivePassReport{segments_written, records_archived}`. The restore target is the existing `ReplayTarget` (`Sequence(u64)` or `Time(String)`) of `crates/medulla-storage/src/recovery/archive.rs`. A scope's archived position is derived from the archive manifest (`ObjectArchive::archived_end`), never stored. Owner: new `crates/medulla-storage/src/recovery/archiver.rs` (R016.1).

### EngineCapabilities

- Fields: `{server_version, wire_features, index_kinds, constraint_kinds, compat_matrix_digest, replication, pitr}`, filled by `EngineCapabilities::for_build` (R020.2). Owner: `crates/medulla-catalog-source-types/src/engine_capabilities.rs`.
