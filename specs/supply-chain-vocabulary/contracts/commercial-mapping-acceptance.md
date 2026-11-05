# Seam: commercial mapping acceptance

Provider: medulla MEDULLA-SUPPLY-VOCAB-R003.13
Consumer: medulla MEDULLA-CQ-CONN-R083

## Wire shape

The commercial module is registered once in the core schema sources with its shapes companion. The connector competency rows map ERPNext, Salesforce and Twenty records onto `kg:Customer`, `kg:CustomerContract` and `kg:Opportunity` and rely on the shared namespace `http://knuckles.team/kg#`.

## Refusal codes

A mapping that narrows a shared property domain or omits `kg:closeDate` on an opportunity is refused by the shapes of `commercial-v1.shapes.ttl`.

## Authoritative types

`crates/medulla-core/ontology/commercial-v1.ttl` and `crates/medulla-core/ontology/commercial-v1.shapes.ttl`.

## Test vectors

`crates/medulla-core/tests/commercial_vocabulary.rs::commercial_module_loads` and `crates/medulla-core/tests/commercial_vocabulary.rs::commercial_shapes_load` are the vectors.
