# MEDULLA-TYPED-PACKS — Quickstart

The shortest served proof imports one connector pack through the production dispatch and reads it back through the typed catalog.

## Scenario

1. Start the served harness of `crates/medulla-h-admin`, which builds a tenant, a principal and a bound importer.
2. Send `ConnectorPack.Import` with a pack of one server, one tool, one skill and one ontology entry; read `Status`.
3. Import the identical pack again and check the result is `Unchanged`; read the component `Content` as an authorized reader and as an unauthorized one.

## Commands

Run the proof for `MEDULLA-TYPED-PACKS-R001`:

```text
cargo test -p medulla-h-admin connector_pack
```

Run the served Python tests of the family:

```text
uv run pytest tests/packs tests/typed_packs -k served
```
