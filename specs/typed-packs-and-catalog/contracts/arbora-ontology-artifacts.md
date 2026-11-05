# Seam: generated ontology and shape artifacts

Provider: medulla MEDULLA-TYPED-PACKS-R090
Consumer: arbora ARBORA-DEPLOY-R009.3

## Wire shape

The typed Rust vocabulary owns the core ontology and shape corpus. The checked-in Turtle under `crates/medulla-core/ontology/` is generated from it deterministically, and arbora deployment consumes the shipped corpus through the packaged read API (`medulla/ontology`). Regeneration reproduces identical bytes across runs.

## Refusal codes

`ONTOLOGY_DRIFT` and `ONTOLOGY_DUPLICATE`.

## Authoritative types

`VocabularyModule`, `OwlClass` and `DatatypeProperty` in `crates/medulla-core/src/ontology_codegen.rs`.

## Test vectors

`crates/medulla-core/src/ontology_codegen.rs::tests::regeneration_is_byte_identical_across_runs_and_to_checked_in_output` and `tests/test_ontology_package.py::test_api_returns_every_source_file`.
