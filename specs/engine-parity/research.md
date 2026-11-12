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
