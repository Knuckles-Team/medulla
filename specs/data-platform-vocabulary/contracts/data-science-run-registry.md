# Seam: data-science run-registry read

Provider: data-science-mcp CONN-DATA-SCIENCE-MCP-DATA-PLATFORM-OPERATIONS-R001
Consumer: medulla MEDULLA-DATAPLAT-VOCAB-R006.8

## Wire shape

A read-only tool returns training runs with their dataset version, validation metrics and baseline run from the in-process run tracker. Medulla maps `load_dataset`, `dataset_lineage`, `rank_models` and `evaluate_model` to `dp:Dataset`, `dp:Model`, `dp:TrainingRun` and `dp:EvaluationResult` through `crates/medulla-core/ontology/fixtures/data_platform/data-science.ttl`; run history is materialised with a stated reason because the registry is in-process.

## Refusal codes

A baseline comparison with no registered baseline run abstains typed and never invents a baseline.

## Authoritative types

`dp:Dataset`, `dp:Model`, `dp:TrainingRun`, `dp:EvaluationResult`, `dp:baselineRun` in `crates/medulla-core/ontology/data_platform-v1.ttl`.

## Test vectors

`crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_data_science_validates`.
