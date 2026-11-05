# Seam: vector collection statistics and get-by-id

Provider: vector-mcp CONN-VECTOR-MCP-DATA-PLATFORM-OPERATIONS-R001
Consumer: medulla MEDULLA-DATAPLAT-VOCAB-R006.4

## Wire shape

`vector_collection_management` gains a `stats` action returning count, dimension, embedding model and distance metric, and a get-by-id action. Medulla maps `vector_collection_management:list_collections` to `dp:VectorCollection` through `crates/medulla-core/ontology/fixtures/data_platform/vector.ttl`; `dp:vectorCount`, `dp:dimensions` and `dp:EmbeddingSpace` are filled once the action exists.

## Refusal codes

A collection without statistics abstains typed on the statistics question. Vectors and chunk text are never mapped to a property.

## Authoritative types

`dp:VectorCollection`, `dp:EmbeddingSpace`, `dp:dimensions`, `dp:vectorCount` in `crates/medulla-core/ontology/data_platform-v1.ttl`.

## Test vectors

`crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_vector_validates`; the connector proof is in `tests/test_mcp_server.py` of the connector repository (planned).
