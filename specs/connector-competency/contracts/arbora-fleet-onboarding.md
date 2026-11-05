# Seam: fleet onboarding handshake

Provider: arbora ARBORA-FLEET-R041.7
Consumer: medulla MEDULLA-CQ-CONN-R085

## Wire shape

When a connector joins the fleet, the arbora multiplexer runs the onboarding handshake and sends the connector's pack to the served `ConnectorPackOp::Import` with the verified tenant and principal. Medulla projects the pack, then answers parent-ontology questions from native rows and live connector rows.

## Refusal codes

An orphaned ontology or a held source abstains by name. A pack the principal may not import returns `ACCESS_DENIED`. The other stable codes are in `MEDULLA-TYPED-PACKS-R106`.

## Authoritative types

The import request and result types in `crates/medulla-catalog-types/src/connector_pack/`. The handshake itself belongs to the arbora spec `ARBORA-FLEET-R041.7`.

## Test vectors

`tests/typed_packs/test_connector_auto_onboard_served.py` (planned) holds the served vectors, driven by a real fixture connector process over MCP.
