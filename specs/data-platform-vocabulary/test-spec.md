# MEDULLA-DATAPLAT-VOCAB — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Unit | load `data_platform-v1.ttl` | all shared, store and mapping IRIs present with domain/range | PENDING |
| T-002 | FR-001 | Unit | validate a `document` connection and an `ftp` one against the canonical virtual-graph shape | `document` accepted, `ftp` rejected | `crates/medulla-shacl/tests/data_platform_vocab.rs::document_source_kind_accepted` |
| T-003 | FR-002 | Unit | `dp:SecretPath` with a `value` triple, then with `token` | both fail `sh:closed` validation | PENDING |
| T-004 | FR-002 | Unit | `dp:StoreMapping` with `dp:viaOperation` `openbao_mcp_kv:kv2_get` | refused by the `dp:StoreMappingShape` denylist | PENDING |
| T-005 | FR-003 | Unit | `materialised` mapping without reason; `virtual` without reason | first rejected, second accepted | PENDING |
| T-006 | FR-004 | Unit | scan vocabulary and fixtures for discovery operations | none declared | PENDING |
| T-007 | FR-005 | Unit | resolve each alignment row | every package term maps to a `dp:` term | PENDING |
| T-008 | FR-006 | Review | Connector rows exist for objectstore, vector, openbao, clarity, data-science | each names an owner repo and a fixture test | PENDING |
| T-009 | FR-002 | Unit | one policy grants read on path A and deny on path B. Then add an invalid permission | each grant retains its policy/path association. Invalid permission rejected | PENDING |

## Negative and boundary cases

Negative proof: an absent source yields a typed abstention, never an empty success. A secret value is never representable. A negative `dp:lag`, a zero `dp:dimensions` and a `presenceRatio` of 2 are rejected.

## Quality and release proof

Focused commands:

- `cargo test -p medulla-core --test data_platform_vocab` checks vocabulary parsing, fixtures and catalog registration.
- `cargo test -p medulla-shacl --test data_platform_vocab` checks the native SHACL acceptance fixture.
- `cargo test -p medulla-plan virtual_graph_mapping` checks the engine-side mirror of the secret-value denylist.

Spec gates: `spec-standard`, `plan-conformance` and `spec-decomposition`.

## Bound tests

Each leaf row binds the tests below. A test marked `(planned)` does not exist yet; the row names its exact path.

| Case | Requirement | Proving test |
|---|---|---|
| Add `dp:DataStore`, `dp:storeFamily`, `dp:inStore`, `dp:viaConnection`, `dp:asOf` to the new vocabulary file | `MEDULLA-DATAPLAT-VOCAB-R001.1` | `crates/medulla-core/tests/data_platform_vocab.rs::shared_terms_present` |
| Add `dp:StoreMapping` (subclass of `vg:VirtualMapping`) with `dp:contractKind`, `dp:viaOperation`, `dp:accelerationMode`, `dp:materialisationReason` | `MEDULLA-DATAPLAT-VOCAB-R001.2` | `crates/medulla-core/tests/data_platform_vocab.rs::store_mapping_subclass_of_virtual_mapping` |
| Add `document` to the `vg:sourceKind` list comment and `sh:in` of the source-connection shape | `MEDULLA-DATAPLAT-VOCAB-R001.3` | `crates/medulla-shacl/tests/data_platform_vocab.rs::document_source_kind_accepted` |
| Register `core:data-platform@1` as a core schema source next to `core:virtual-graph@1` | `MEDULLA-DATAPLAT-VOCAB-R001.4` | `crates/medulla-core/tests/data_platform_vocab.rs::core_source_registered` |
| Add `dp:Database`, `dp:Schema`, `dp:Table`, `dp:View`, `dp:Column`, `dp:ForeignKey` | `MEDULLA-DATAPLAT-VOCAB-R002.1` | `crates/medulla-core/tests/data_platform_vocab.rs::relational_classes_present` |
| Add `dp:inSchema`, `dp:hasColumn`, `dp:referencesTable`, `dp:isPrimaryKey`, `dp:sqlDialect` with domain/range | `MEDULLA-DATAPLAT-VOCAB-R002.2` | `crates/medulla-core/tests/data_platform_vocab.rs::relational_properties_domain_range` |
| Add `dp:Collection`, `dp:DocumentField`, `dp:fieldPath`, `dp:fieldType`, `dp:presenceRatio`, `dp:documentCount` | `MEDULLA-DATAPLAT-VOCAB-R002.3` | `crates/medulla-core/tests/data_platform_vocab.rs::document_terms_present` |
| Add a shape: `dp:fieldType` is in the `DocumentFieldType` list (incl. `Mixed`); `dp:presenceRatio` in [0,1] | `MEDULLA-DATAPLAT-VOCAB-R002.4` | `crates/medulla-core/tests/data_platform_vocab.rs::document_field_shape_rejects_ratio_2` |
| Add `dp:Bucket`, `dp:StoredObject`, `dp:objectKey`, `dp:byteSize`, `dp:lastModified`; no content property | `MEDULLA-DATAPLAT-VOCAB-R003.1` | `crates/medulla-core/tests/data_platform_vocab.rs::object_terms_present_without_content_property` |
| Add `dp:VectorCollection`, `dp:EmbeddingSpace`, `dp:embeddingModel`, `dp:dimensions`, `dp:distanceMetric`, `dp:usesEmbeddingSpace`, `dp:vectorCount` | `MEDULLA-DATAPLAT-VOCAB-R003.2` | `crates/medulla-core/tests/data_platform_vocab.rs::vector_terms_present` |
| Add a shape: `dp:dimensions` positive integer; exactly one embedding space per vector collection | `MEDULLA-DATAPLAT-VOCAB-R003.3` | `crates/medulla-core/tests/data_platform_vocab.rs::vector_dimension_zero_rejected` |
| Add `dp:Topic`, `dp:Partition`, `dp:ConsumerGroup`, `dp:PartitionLag`, `dp:partitionOf`, `dp:subscribesTo`, `dp:lag`, `dp:groupState` | `MEDULLA-DATAPLAT-VOCAB-R004.1` | `crates/medulla-core/tests/data_platform_vocab.rs::stream_terms_present` |
| Add a shape: `dp:lag` integer >= 0 and `dp:asOf` required on `dp:PartitionLag` | `MEDULLA-DATAPLAT-VOCAB-R004.2` | `crates/medulla-core/tests/data_platform_vocab.rs::negative_lag_rejected` |
| Add `dp:AnalyticsSite`, `dp:BehaviorMetric`, `dp:metricName`, `dp:metricValue`, `dp:windowDays`, `dp:dimensionName`, `dp:dimensionValue` | `MEDULLA-DATAPLAT-VOCAB-R004.3` | `crates/medulla-core/tests/data_platform_vocab.rs::analytics_terms_present` |
| Add `dp:Dataset` as a subclass of the existing core `:Dataset`, `dp:Model` as a general trained-model class (not equivalent to the narrower LLM/embedding `:Model`), `dp:TrainingRun`, `dp:EvaluationResult`, `dp:trainedOn`, `dp:producedModel`, `dp:evaluatedOn`, `dp:baselineRun` | `MEDULLA-DATAPLAT-VOCAB-R004.4` | `crates/medulla-core/tests/data_platform_vocab.rs::ml_terms_present` |
| Add `dp:SecretEngine`, `dp:SecretPath`, `dp:AccessPolicy`, `dp:RotationState`, `dp:hasRotationState`, `dp:lastRotatedAt`, `dp:currentVersion`, `dp:pathName`, `dp:PolicyPathGrant`, `dp:grantPolicy`, `dp:grantPath`, `dp:grantsCapability`, `dp:leaseExpiresAt` | `MEDULLA-DATAPLAT-VOCAB-R005.1` | `crates/medulla-core/tests/data_platform_vocab.rs::secret_metadata_terms_present` |
| Add closed shapes (`sh:closed true`) for the five secret-metadata classes, including `dp:PolicyPathGrant` | `MEDULLA-DATAPLAT-VOCAB-R005.2` | `crates/medulla-core/tests/data_platform_vocab.rs::secret_path_with_extra_property_rejected` |
| Refusal test row: a `dp:SecretPath` carrying a `value` (and one carrying `token`) triple fails validation, and the vocabulary file contains no property named value, secret, token, password, ciphertext or rules | `MEDULLA-DATAPLAT-VOCAB-R005.3` | `crates/medulla-core/tests/data_platform_vocab.rs::secret_value_refused_and_no_value_property` |
| Add a mapping-validator rule that refuses a `dp:StoreMapping` whose `dp:viaOperation` is `openbao_mcp_kv:kv2_get`, `kv1_get`, `openbao_mcp_logical:read` or `unwrap` | `MEDULLA-DATAPLAT-VOCAB-R005.4` | `crates/medulla-core/tests/data_platform_vocab.rs::openbao_value_operation_mapping_refused`; `crates/medulla-plan/src/virtual_graph_mapping.rs::tests::value_operations_are_refused` |
| Add a shape: each `dp:PolicyPathGrant` has exactly one `dp:grantPolicy` (`dp:AccessPolicy`), exactly one `dp:grantPath` (`dp:SecretPath`), and at least one `dp:grantsCapability` string in `read list create update delete sudo deny` | `MEDULLA-DATAPLAT-VOCAB-R005.5` | `crates/medulla-core/tests/data_platform_vocab.rs::grants_capability_closed_list` |
| Add the `sql` `dp:StoreMapping` fixture for `sql_schema:tables`, `columns`, `foreign_keys`, `constraints` with `dp:accelerationMode` V (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.1` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_sql_validates` |
| Add the `documentdb` `dp:StoreMapping` fixture for `documentdb_collections:list_collections`, `documentdb_analysis:aggregate` with `dp:accelerationMode` V (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.2` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_documentdb_validates` |
| Add the `objectstore` `dp:StoreMapping` fixture for `buckets:list`, `objects:list`, `objects:head` with `dp:accelerationMode` V (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.3` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_objectstore_validates` |
| Add the `vector` `dp:StoreMapping` fixture for `vector_collection_management:list_collections` with `dp:accelerationMode` V (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.4` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_vector_validates` |
| Add the `kafka` `dp:StoreMapping` fixture for `kafka_groups:lags`, `kafka_partitions:list`, `kafka_topics:list` with `dp:accelerationMode` V; M for sustained lag (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.5` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_kafka_validates` |
| Add the `openbao` `dp:StoreMapping` fixture for `openbao_mcp_sys:get_mounts`, `openbao_mcp_logical:list` with `dp:accelerationMode` V (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.6` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_openbao_validates` |
| Add the `clarity` `dp:StoreMapping` fixture for `clarity_insights:get_data_export` with `dp:accelerationMode` V up to 3 days; M beyond (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.7` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_clarity_validates` |
| Add the `data-science` `dp:StoreMapping` fixture for `load_dataset`, `dataset_lineage`, `rank_models`, `evaluate_model` with `dp:accelerationMode` V; M for run history (reason recorded where M) | `MEDULLA-DATAPLAT-VOCAB-R006.8` | `crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_data_science_validates` |
| Add a shape rule: `materialised` without `dp:materialisationReason` is a violation; `virtual` needs none | `MEDULLA-DATAPLAT-VOCAB-R007.1` | `crates/medulla-core/tests/data_platform_vocab.rs::materialised_without_reason_rejected` |
| Add a shape rule: `contractKind = mcp-operation` requires `dp:viaOperation` | `MEDULLA-DATAPLAT-VOCAB-R007.2` | `crates/medulla-core/tests/data_platform_vocab.rs::mcp_operation_requires_via_operation` |
| Add a test that no `dp:` term or mapping fixture declares a discovery operation: relational/document entries come from `vg:MetadataContract` only | `MEDULLA-DATAPLAT-VOCAB-R007.3` | `crates/medulla-core/tests/data_platform_vocab.rs::no_second_discovery_path` |
| Add a test that every fixture mapping not marked `materialised` has no `AccelerationPolicy` and every `materialised` one cites R038 | `MEDULLA-DATAPLAT-VOCAB-R007.4` | `crates/medulla-core/tests/data_platform_vocab.rs::virtual_default_for_all_eight` |
| Add an alignment table (package term -> `dp:` term, `rdfs:subClassOf` / `owl:equivalentProperty`) for sql and documentdb | `MEDULLA-DATAPLAT-VOCAB-R008.1` | `crates/medulla-core/tests/data_platform_vocab.rs::alignment_relational_resolves` |
| Add the alignment table for objectstore and vector | `MEDULLA-DATAPLAT-VOCAB-R008.2` | `crates/medulla-core/tests/data_platform_vocab.rs::alignment_object_vector_resolves` |
| Add the alignment table for kafka, openbao, clarity, data-science | `MEDULLA-DATAPLAT-VOCAB-R008.3` | `crates/medulla-core/tests/data_platform_vocab.rs::alignment_ops_ml_resolves` |
