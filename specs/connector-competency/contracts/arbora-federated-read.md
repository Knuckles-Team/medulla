# Seam: federated read through the served door

Provider: medulla MEDULLA-CQ-CONN-R090
Consumer: arbora ARBORA-ACCEPTANCE-R006.7

## Wire shape

Arbora `ask` sends the question as served `UnifiedQuery`. Medulla plans source selection from the ontology, reads two virtual-graph connectors and the engine facts, checks the caller's access on each source before reading it, and returns rows that each cite their source. The query is read-only.

## Refusal codes

A query with a write stage refuses before any source is called. A source the caller cannot read adds no rows and appears in the coverage block (`MEDULLA-SRC-AUTH-R006.1`).

## Authoritative types

The `UnifiedQuery` request and result types in `crates/medulla-protocol-types/src/protocol/`. The registry is `ForeignSourceRegistry` in `crates/medulla-federation/src/federation.rs`.

## Test vectors

`tests/federated/test_virtual_graph_federation_served.py` (planned) holds the served vectors that both sides test against.
