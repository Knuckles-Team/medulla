# MEDULLA-DECISION-ENGINE — Quickstart

The shortest served proof of the decision ladder drives one question per resolution kind through the production dispatch.

## Scenario

1. Start the served harness of `crates/medulla-h-decide`, which builds a tenant, a principal and a candidate set.
2. Send one `Decide` or `AgentAssemble` request for each kind: constraint, entailment, optimization, statistical and abstention.
3. Read the sealed record of each answer and check its `ResolutionKind`, its `EvidenceClass` and its premises.

## Commands

Run the proof for `MEDULLA-DECISION-ENGINE-R001`:

```text
cargo test -p medulla-h-decide every_resolution_kind_is_reachable_through_production_dispatch
```

Run the served Python tests of the family:

```text
uv run pytest tests/decide -k served
```
