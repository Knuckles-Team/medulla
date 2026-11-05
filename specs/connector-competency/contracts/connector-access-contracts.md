# Seam: connector access contracts and mappings

Provider: the connector repositories, rows `CONN-<REPO>-CONNECTOR-CONTENT-R002` of each connector
Consumer: medulla MEDULLA-CQ-CONN-R067

## Wire shape

Each connector ships an ontology pack with its access contract (MCP, A2A, API, GraphQL or virtual graph over a relational or document store), its ingest contract and its mapping. Medulla imports the pack through `ConnectorPackOp::Import`, approves the mapping, and reads the source at query time through `foreign_scan`. Medulla stores the pack and catalog nodes only, never the source rows.

## Refusal codes

Medulla refuses with `SOURCE_REGISTRATION_NOT_FOUND` when the pack is not imported, `SOURCE_HELD` when the mapping is held, and `SOURCE_UNAVAILABLE` when the source is down. An unconnected source abstains with reason `Unconnected`.

## Authoritative types

The pack and catalog types in `crates/medulla-catalog-types/src/connector_pack/`. The recording and case schema is the case loader of `MEDULLA-CQ-CONN-R067.7`.

## Test vectors

`tests/connector_competency/cases/<connector>.json` and `tests/connector_competency/recordings/<connector>/` hold the vectors that the connector repository and medulla both replay (planned).
