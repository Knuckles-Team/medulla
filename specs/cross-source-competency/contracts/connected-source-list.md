# Seam: the connected-source list for the live run

Provider: arbora ARBORA-FLEET-R052.2
Consumer: medulla MEDULLA-CQ-XSRC-R007.2

## Wire shape

Arbora reports which fleet connectors are connected on the day of the live run. Medulla compares that list with the authority rows and reports each unconnected authoritative source as an `Unconnected { source, class }` abstention. The run records the report, the sources used and the abstentions in the evidence folder of this spec.

## Refusal codes

A source absent from the list is an abstention, not a refusal. A caller that cannot read a source gets `RoleInsufficient` in the coverage block (`MEDULLA-SRC-AUTH-R006.1`).

## Authoritative types

The connected view is `ConnectedSources` in `crates/medulla-federation/src/federation.rs` (new in `MEDULLA-SRC-AUTH-R004.6`), built from the owner-scoped foreign catalog.

## Test vectors

`tests/federated/test_supply_chain_impact_served.py::test_report_from_ingested_connector_rows` (planned).
