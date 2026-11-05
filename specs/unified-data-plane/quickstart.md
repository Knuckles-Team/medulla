# MEDULLA-UNIFIED-DATA-PLANE — Quickstart

The shortest served proof attaches one source and answers one question over it through the production dispatch.

## Scenario

1. Start the served harness with a tenant, a principal and a recorded Postgres-wire source.
2. Register the source through `Method::RegisterForeignSource` with a secret reference and read its hashed catalog graph.
3. Approve one R2RML mapping and run one SPARQL question and one `UnifiedQuery` over the named virtual graph.
4. Read EXPLAIN and check the pushed fragments, the snapshot and the absence of a stored entity row.

## Commands

Run the registry and virtual-graph proof:

```text
uv run pytest tests/test_uql_foreign_execution.py -k one_registered_source
```

Run the federation proofs of the source categories:

```text
uv run pytest tests/federation -k served
```

Run the Rust proofs of the typed models:

```text
cargo test -p medulla-types metadata_acceleration
```
