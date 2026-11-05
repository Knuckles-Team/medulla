# MEDULLA-ENGINE-PARITY — Quickstart

The shortest served proof connects a PostgreSQL client to a loopback engine and exercises the main flows.

## Scenario

1. Start a loopback engine with one tenant and a wire login that maps to an engine principal.
2. Connect with `tokio-postgres`, run a prepared statement, `COPY` in and out, and read a tagged table.
3. Run two sessions that update one row and assert SQLSTATE `40001` for the second.

## Commands

Run the existing OCC proof of `MEDULLA-ENGINE-PARITY-R002.1`:

```text
cargo test -p medulla-transaction --test occ
```

Run the wire proof of `MEDULLA-ENGINE-PARITY-R006.1` (planned):

```text
cargo test --test pgwire_roundtrip extended_query_prepared_and_copy
```

Run the compatibility conformance of `MEDULLA-ENGINE-PARITY-R009.3` (planned):

```text
uv run pytest tests/test_sql_compat_conformance.py
```
