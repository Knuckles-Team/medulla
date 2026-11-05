# MEDULLA-INGEST-WORKER — Quickstart

The shortest served proof drives one job through the queue and one batch through `SourceIngest`.

## Scenario

1. Start a disposable local Medulla instance with the served Python harness. It creates a tenant, a principal and a graph.
2. Register a worker, enqueue one job twice, claim it, fail it past `max_attempts` and read the dead-letter record.
3. Send one document batch through `SourceIngest`, then read the atom context and check the provenance keys.

## Commands

Run the unit proofs of the job types:

```text
cargo test -p medulla-source-types ingest_job
```

Run the served proofs of the queue and of the admission path:

```text
uv run pytest tests/ingest -k served
```
