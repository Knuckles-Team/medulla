# Contract — scion-data-locations

Provider: scion-sdk SCION-CONNECTOR-CONTROL-R916.3
Consumer: medulla MEDULLA-SRC-AUTH-R013

The pipelines gate row `PIPE-GATES-R014.7` requires every connector to declare the locations.

## Wire shape

A pack's `contracts.ttl` holds `sc:DataLocation` individuals (`sc:` = `https://knuckles-team.github.io/scion-sdk/contract#`). Each has `sc:holdsClass` (one or more class IRIs), `sc:instance`, optional `sc:store`, `sc:readTool`, optional `sc:routeParam` and optional `sc:federationKey`.

## Refusal codes

`DATA_LOCATION_INCOMPLETE`: a location with no held class or no instance yields no authority row and one audit entry.

## Authoritative types

The SDK builder is `DataLocation` in `scion_sdk/surface_contracts.py`. The engine mapping is `absorb` in `crates/medulla-virtual-graph/src/source_selection/live.rs`.

## Test vectors

`crates/medulla-rdf/tests/source_authority_fixture.rs::data_location_yields_a_proposed_pack_row` (planned).
