# Seam: connector class resolution

Provider: medulla MEDULLA-SUPPLY-VOCAB-R005
Consumer: connectors CONN-ERPNEXT-AGENT-CONNECTOR-CONTENT-R002 and CONN-SALESFORCE-AGENT-CONNECTOR-CONTENT-R002

## Wire shape

A connector manifest resolves each `schema_mappings.<X>.ontology_class` to a class IRI declared by this spec, such as `kg:Supplier`, `kg:SupplyChainItem`, `kg:Customer` and `kg:CustomerContract`. Medulla holds one `vg:VirtualMapping` fixture per source concept; the connector holds the access contract.

## Refusal codes

An unresolved class makes the concept uncovered, and the report abstains with the typed `Uncovered` verdict; no new code.

## Authoritative types

The classes of `crates/medulla-core/ontology/supply_chain-v1.ttl` and `crates/medulla-core/ontology/commercial-v1.ttl`.

## Test vectors

The fixtures under `crates/medulla-core/tests/fixtures/` named by the R005 rows are the vectors; each connector proves its manifest against the same class IRIs.
