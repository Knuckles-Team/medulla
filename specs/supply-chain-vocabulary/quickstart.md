# MEDULLA-SUPPLY-VOCAB — Quickstart

The shortest proof loads the default schema catalog, runs the native reasoner over the physical chain fixture and reads the ecosystem impact through the served method.

## Scenario

1. Load `GraphSchemaSources::default()`; the `supply_chain`, `commercial` and `infrastructure` core sources compose.
2. Run the reasoner over the four-edge service chain and read the inferred `kg:suppliesService` edge.
3. Send `Method::MineRiskPropagation` with an `ecosystem` query for a leaf package and read every dependent with its path and severity.

## Commands

Run the declaration, reasoner and shape tests:

```text
cargo test -p medulla-rdf --test supply_chain_declarations --test supply_chain_reasoning --test commercial_declarations
cargo test -p medulla-core --test supply_chain_vocab --test commercial_vocabulary --test ecosystem_dependency
```

Run the served ecosystem impact proof:

```text
cargo test -p medulla-h-mining served_impact
```
