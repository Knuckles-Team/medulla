# MEDULLA-ENGINE-PARITY — Data model

This file describes the entities that the rows add or extend.

## Entities

### TxnFootprint, KeyRange and CommittedWrites

- Fields: `TxnFootprint{snapshot_seq, reads, writes}`; `KeyRange` is a table and a key range; `CommittedWrites` is the writes of one commit.
- Owner: `crates/medulla-transaction/src/occ.rs`. `TableStore` keeps a ring of the last `tables.occ_history_commits` commits.

### SqlFormEntry and SqlFormState

- Fields: `SqlFormEntry{name, kind, state}`; `kind` is `Statement`, `Function`, `Operator` or `Type`; `SqlFormState` is `Supported`, `Partial` or `Refused`.
- Owner: `crates/medulla-types/src/pg_operational_surface.rs`, inside `PgCompatibilityMatrix`. The matrix is closed: an absent form is `Refused`.

### SQLSTATE table

- One closed table from stable engine codes to SQLSTATE values, in new `crates/medulla-wire-core/src/wire/sqlstate.rs`. An unknown code keeps `58000`.

### WireLoginRecord and WireLoginKind

- Fields: `WireLoginRecord{login, kind, verifier, principal, tenant, created_ms, revoked_ms}`; `WireLoginKind` is `Password`, `Certificate` or `Token`.
- Owner: new `crates/medulla-identity-types/src/identity/wire_login.rs`. No secret is stored; the verifier is a stored key or a digest.

### SecondaryIndexSpec extension

- Adds `predicate: Option<CheckExpr>` and, per column, `expr: Option<IndexExpr>` (`Lower`, `Upper`, `Abs`, `Length`). Owner: `crates/medulla-tables/src/index.rs`. The schema version moves to 2.

### TriggerSpec and TriggerReaction

- Fields: `TriggerSpec{table, name, timing, events, machine_id}`; `TriggerReaction{event_id, trigger, statement_id}` in an append-only ledger.
- Owner: new `crates/medulla-table-store/src/tables/trigger.rs`.

### Sequence

- Fields: `Sequence{name, start, increment, cache}`; stored in the table `sql_sequences` as `{high_mark, last_issued}`. Owner: new `crates/medulla-table-store/src/tables/sequence.rs`.

### MatViewSpec

- Fields: `MatViewSpec{name, query, inputs, refreshed_seq, stale}`; stored in the catalog table `sql_matviews`, with the result rows in a user table. Owner: new `crates/medulla-table-store/src/tables/matview.rs`.

### CdcEvent extension

- Adds the kinds `InsertRow`, `UpdateRow`, `DeleteRow` to `CdcKind` and a `table` field to `CdcEvent`. Owner: `crates/medulla-types/src/wire_streaming.rs`.

### RecoveryTarget and ArchivePolicy

- Fields: `RecoveryTarget` is `Seq(u64)` or `TimeMs(u64)`; `ArchivePolicy{segment_bytes, retain_s, interval_s}`. Owner: new `crates/medulla-storage/src/recovery/archive.rs`.

### EngineCapabilities

- Fields: `{wire_features, index_kinds, constraint_kinds, compat_matrix_digest, replication, pitr}`. Owner: `crates/medulla-types/src/engine_capabilities.rs`.
