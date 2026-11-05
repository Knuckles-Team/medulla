# Seam: objectstore last-access operation

Provider: objectstore-mcp CONN-OBJECTSTORE-MCP-DATA-PLATFORM-OPERATIONS-R001
Consumer: medulla MEDULLA-DATAPLAT-VOCAB-R006.3

## Wire shape

The `objects` tool returns a `last_accessed` field on `head` and `list` results, or a typed `unsupported` result for a backend without one. Medulla maps `objects:list` and `objects:head` to `dp:StoredObject` (`dp:lastModified`) through the fixture `crates/medulla-core/ontology/fixtures/data_platform/objectstore.ttl`; the single-source question answers with `lastModified` and says so until the field exists.

## Refusal codes

A backend without last-access data returns `unsupported`; the competency question then abstains typed and never reports an invented age. Object bytes are never mapped.

## Authoritative types

`dp:StoredObject`, `dp:objectKey`, `dp:byteSize` and `dp:lastModified` in `crates/medulla-core/ontology/data_platform-v1.ttl`.

## Test vectors

`crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_objectstore_validates`; the connector proof is `tests/test_last_access_operation.py` in the connector repository (planned).
