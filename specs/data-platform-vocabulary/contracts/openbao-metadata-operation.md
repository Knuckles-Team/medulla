# Seam: openbao metadata-only operation

Provider: openbao-mcp CONN-OPENBAO-MCP-DATA-PLATFORM-OPERATIONS-R001
Consumer: medulla MEDULLA-DATAPLAT-VOCAB-R006.6

## Wire shape

The secrets tool gains a `metadata` action returning the KV v2 path, current version and created and updated time, and a policy-to-path capability matrix, with no value and no policy text. Medulla maps `openbao_mcp_sys:get_mounts` and `openbao_mcp_logical:list` to `dp:SecretEngine` and `dp:SecretPath` through `crates/medulla-core/ontology/fixtures/data_platform/openbao.ttl`.

## Refusal codes

The value operations `openbao_mcp_kv:kv2_get`, `kv1_get`, `openbao_mcp_logical:read` and `unwrap` are refused by `dp:StoreMappingShape` and never mapped. Until the action exists the rotation and grant questions abstain typed.

## Authoritative types

`dp:SecretEngine`, `dp:SecretPath`, `dp:AccessPolicy`, `dp:RotationState`, `dp:PolicyPathGrant` in `crates/medulla-core/ontology/data_platform-v1.ttl`.

## Test vectors

`crates/medulla-core/tests/data_platform_vocab.rs::openbao_value_operation_mapping_refused` and `::mapping_fixture_openbao_validates`.
