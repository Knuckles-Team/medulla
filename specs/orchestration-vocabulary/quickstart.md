# MEDULLA-ORCH-VOCAB — Quickstart

The shortest proofs drive the vocabulary through the reasoner and through the served write and read paths.

## Scenario

1. Load the orchestration ontology, its shapes and the deploy-container fixture into the reasoner test module.
2. Send a shape-valid `:RunTrace` through `add_node`, then read it back by Cypher and by a `Method::Uql` metric statement.
3. Index a fixture spec folder through a scoped `IndexRepository` and list the typed nodes with their deciding rules.

## Commands

Run the vocabulary and shape tests for `MEDULLA-ORCH-VOCAB-R001.1`:

```text
cargo test -p medulla-rdf --features owl orchestration_vocabulary
```

Run the served Python tests of the family:

```text
uv run pytest tests/orchestration -k served
```
