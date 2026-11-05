# Seam: data-prep declaration from the arbora host

Provider: medulla MEDULLA-INGEST-WORKER-R001.1008
Consumer: arbora ARBORA-FLEET-R058

## Wire shape

The host sends `DataPrepDeclare` with a capability, sorted opaque field references and a policy digest, and reads the stored set with `DataPrepDeclarationRead`. The response is an `IngestBatchReport`.

## Refusal codes

`DATA_PREP_DECLARATION_INVALID` for a value outside the grammar or an unknown key. `DATA_PREP_DECLARATION_TOO_LARGE` for more than 256 field references.

## Authoritative types

`DataPrepDeclaration` in `crates/medulla-types/src/data_prep_declaration.rs`.

## Test vectors

`tests/ingest/test_data_prep_declaration_served.py::test_declare_then_read_through_the_python_client` matches the Rust round-trip vectors.
