# Research: engine parity

Decisions that shape the rows. Current state only.

## Sequences persist in the existing allocator table

A named sequence needs durable state, and a new owner table is a storage layout
change with a frozen predecessor census and an offline upgrade. The per-table
`__sql_seq__` allocator already holds `name -> u64`. A sequence is four `u64`
entries under a NUL-prefixed key that no table name can equal, written in one
maintenance transaction. `R013.2` moves the serial counters onto the same keys.
A later layout change may give sequences their own table.

## One SQLSTATE table, shared by the wire and the store errors

`crates/medulla-wire-core/src/wire/sqlstate.rs` is the only table from engine
code to SQLSTATE. `user_err` of the wire core and of the pgwire front end both
read it, so a store error that starts with `<CODE>: ` reaches the client with its
state and an unlisted code keeps `58000`.

## COPY out is a plain read

`COPY <table> TO STDOUT` runs `SELECT <columns> FROM <table>` through the
authorized read path, so access control and masking are those of a `SELECT` and
no second policy path exists. Binary COPY out is refused with
`FEATURE_NOT_SUPPORTED`.

## Partial and expression indexes are never a plain lookup

A partial index omits rows and an expression index keys a derived value, so a
first-column lookup or an ordered read on the raw column ignores them. Only a
planner that proves the query implies the predicate (`R010.2`) uses a partial
index.

## Benchmark suite ordering

The conflict-heavy workload of `R005.1` needs optimistic commit validation, so
`R005.1` depends on `R002.2`. `R001.3` keeps its nightly group in the same suite
and lands with `R005.1`.

## Decision: where the footprint is validated and how the history is configured (R002.2)

The keys a transaction reads and writes are the rows its `UPDATE` and `DELETE` matched and the rows its inserts and updates wrote, and the store only knows them once the ops run inside the write transaction. Validation therefore runs there, after the ops are staged and before the commit point, and a conflict aborts the transaction; redb discards every staged row, so nothing is written. This is exact and needs no second scan of the table. The write set is recorded in the ring inside the same serialized write transaction, so no commit validates between the record and the commit point; a failure before the commit point retracts the record. A pinned footprint older than the ring refuses with `TXN_SNAPSHOT_EXPIRED`. The history size follows the precedent of the enqueue timeout of R001.1: an environment variable (`MEDULLA_TABLES_OCC_HISTORY_COMMITS`) that parses and clamps into the documented bounds. The `ON CONFLICT` insert path is not a buffered `TxnOp`, so it runs auto-commit and never carries a footprint.

## Decision: row changes reach the feed through the tenant store (R015.1)

The change feed is the one `CdcHub`; the table store owns no feed. The hub is the store's `RowChangeSink`, installed by `handle_sql` on the tenant store before any statement runs (the store keeps the first sink it is given, so a repeat install is a no-op). A feed event of a row is one `Row { table, operation, key }` change on `ReadGraphChanges`, never a second method. Reading rows needs the `cdc:read` scope and read access to the table (`sql_catalog_acl::may_stream_table`: owner, admin or SELECT grantee, and no row-level column, because the stream carries every row and a row-level table is filtered per caller). The ring size is the registry knob `cdc.ring_events`, replacing the `MEDULLA_CDC_RING` environment variable, so one registry owns the tunable and a served test can move a cursor out of the window with a bounded number of commits.
