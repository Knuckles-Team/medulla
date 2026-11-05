# Seam: connector onboarding and seed graphs

Provider: medulla MEDULLA-TYPED-PACKS-R104.3
Consumer: arbora ARBORA-FLEET-R036.3

## Wire shape

Arbora calls the served `ConnectorPack.import` with a connector pack that may carry a seed body, and later the served `AddTriples` for the onboarding pass (`ARBORA-FLEET-R041.17`). Medulla validates the pack, commits it in one owner transaction and writes the seed triples and the seed revision node with the pack graph in one commit. Arbora supplies the verified principal and tenant; Medulla never reads a principal from the request body.

## Refusal codes

`ONTOLOGY_ORPHAN` and `ONTOLOGY_PACK_IMPORTS_CORE` (`MEDULLA-TYPED-PACKS-R105`), plus the pack violation codes registered in `ServerErrorCode`. A refused import leaves durable state unchanged.

## Authoritative types

`ConnectorPackOp` and the pack result types in `crates/medulla-catalog-types/src/connector_pack/`. The seed wire types are `SeedPart` and `SeedGraphImport` in `crates/medulla-catalog-types/src/connector_pack/seed_graph.rs`; the plan is `SeedImportPlan` in `crates/medulla-h-admin/src/admin/connector_pack/seed.rs`.

## Test vectors

`crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::served_import_projects_seed` is the served vector; the arbora side replays the same pack bytes.
