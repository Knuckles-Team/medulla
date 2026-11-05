# MEDULLA-DATAPLAT-VOCAB — Quickstart

The shortest proof of the vocabulary loads the module, validates the shapes and checks the fixtures. It runs without network.

## Scenario

1. Load the `data-platform` core source through `GraphSchemaSources::default()` and check that the shared, store and mapping terms resolve.
2. Validate a `document` source connection and an `ftp` one against the virtual-graph shape.
3. Feed a `dp:SecretPath` with a `value` triple and a `dp:StoreMapping` that reads a secret value, and check that both are refused.
4. Validate the eight mapping fixtures and the three alignment tables.

## Commands

Run the vocabulary, shape, fixture and alignment tests:

```text
cargo test -p medulla-core --test data_platform_vocab
```

Run the native SHACL acceptance of the `document` source kind:

```text
cargo test -p medulla-shacl --test data_platform_vocab
```

Run the engine-side mirror of the secret-value denylist:

```text
cargo test -p medulla-plan virtual_graph_mapping
```
