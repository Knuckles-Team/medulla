# MEDULLA-CQ-ORCH — Quickstart

The shortest proof of the competency questions loads each question's fixture, runs its query and asserts the expected rows with their source. The served surfaces are proved through the production dispatch.

## Scenario

1. Load the fixture graph of a question with the native parser and run its `query.rq` with the task bound.
2. Assert the rows equal `expected.tsv` and that an unconnected source gives the typed abstention.
3. For a served surface, start the served harness, import the fixture connector packs through `ConnectorPack.Import` and call the served method under a tenant and principal context.

## Commands

Run the question proofs:

```text
cargo test -p medulla-rdf --test orchestration_competency --test orchestration_competency_r0061 --test orchestration_competency_r0071
cargo test -p medulla-decision-outcome-types execution_answer
```

Run the served Python tests once they exist:

```text
uv run pytest tests/orchestration -k served
```
