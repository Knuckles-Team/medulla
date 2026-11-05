# MEDULLA-DATAPLAT-VOCAB — Tasks

One task per leaf row of `requirements.md`, producers first: shared terms and registration, store terms, secret refusal, mapping fixtures, guards, alignment. Each task names its row; the row holds the caller chain, the shape and the test.

## Tasks

- [ ] T001 [US1] MEDULLA-DATAPLAT-VOCAB-R001.1 Add `dp:DataStore`, `dp:storeFamily`, `dp:inStore`, `dp:viaConnection`, `dp:asOf` to the new vocabulary file
- [ ] T002 [US1] MEDULLA-DATAPLAT-VOCAB-R001.2 Add `dp:StoreMapping` (subclass of `vg:VirtualMapping`) with `dp:contractKind`, `dp:viaOperation`, `dp:accelerationMode`, `dp:materialisationReason`
- [ ] T003 [US1] MEDULLA-DATAPLAT-VOCAB-R001.3 Add `document` to the `vg:sourceKind` list comment and `sh:in` of the source-connection shape
- [ ] T004 [US1] MEDULLA-DATAPLAT-VOCAB-R001.4 Register `core:data-platform@1` as a core schema source next to `core:virtual-graph@1`
- [ ] T005 [US1] MEDULLA-DATAPLAT-VOCAB-R002.1 Add `dp:Database`, `dp:Schema`, `dp:Table`, `dp:View`, `dp:Column`, `dp:ForeignKey`
- [ ] T006 [US1] MEDULLA-DATAPLAT-VOCAB-R002.2 Add `dp:inSchema`, `dp:hasColumn`, `dp:referencesTable`, `dp:isPrimaryKey`, `dp:sqlDialect` with domain/range
- [ ] T007 [US1] MEDULLA-DATAPLAT-VOCAB-R002.3 Add `dp:Collection`, `dp:DocumentField`, `dp:fieldPath`, `dp:fieldType`, `dp:presenceRatio`, `dp:documentCount`
- [ ] T008 [US1] MEDULLA-DATAPLAT-VOCAB-R002.4 Add a shape: `dp:fieldType` is in the `DocumentFieldType` list (incl. `Mixed`); `dp:presenceRatio` in [0,1]
- [ ] T009 [US1] MEDULLA-DATAPLAT-VOCAB-R003.1 Add `dp:Bucket`, `dp:StoredObject`, `dp:objectKey`, `dp:byteSize`, `dp:lastModified`; no content property
- [ ] T010 [US1] MEDULLA-DATAPLAT-VOCAB-R003.2 Add `dp:VectorCollection`, `dp:EmbeddingSpace`, `dp:embeddingModel`, `dp:dimensions`, `dp:distanceMetric`, `dp:usesEmbeddingSpace`, `dp:vectorCount`
- [ ] T011 [US1] MEDULLA-DATAPLAT-VOCAB-R003.3 Add a shape: `dp:dimensions` positive integer; exactly one embedding space per vector collection
- [ ] T012 [US1] MEDULLA-DATAPLAT-VOCAB-R004.1 Add `dp:Topic`, `dp:Partition`, `dp:ConsumerGroup`, `dp:PartitionLag`, `dp:partitionOf`, `dp:subscribesTo`, `dp:lag`, `dp:groupState`
- [ ] T013 [US1] MEDULLA-DATAPLAT-VOCAB-R004.2 Add a shape: `dp:lag` integer >= 0 and `dp:asOf` required on `dp:PartitionLag`
- [ ] T014 [US1] MEDULLA-DATAPLAT-VOCAB-R004.3 Add `dp:AnalyticsSite`, `dp:BehaviorMetric`, `dp:metricName`, `dp:metricValue`, `dp:windowDays`, `dp:dimensionName`, `dp:dimensionValue`
- [ ] T015 [US1] MEDULLA-DATAPLAT-VOCAB-R004.4 Add `dp:Dataset` as a subclass of the existing core `:Dataset`, `dp:Model` as a general trained-model class (not equivalent to the narrower LLM/embedding `:Model`), `dp:TrainingRun`, `dp:EvaluationResult`, `dp:trainedOn`, `dp:producedModel`, `dp:evaluatedOn`, `dp:baselineRun`
- [ ] T016 [US2] MEDULLA-DATAPLAT-VOCAB-R005.1 Add `dp:SecretEngine`, `dp:SecretPath`, `dp:AccessPolicy`, `dp:RotationState`, `dp:hasRotationState`, `dp:lastRotatedAt`, `dp:currentVersion`, `dp:pathName`, `dp:PolicyPathGrant`, `dp:grantPolicy`, `dp:grantPath`, `dp:grantsCapability`, `dp:leaseExpiresAt`
- [ ] T017 [US2] MEDULLA-DATAPLAT-VOCAB-R005.2 Add closed shapes (`sh:closed true`) for the five secret-metadata classes, including `dp:PolicyPathGrant`
- [ ] T018 [US2] MEDULLA-DATAPLAT-VOCAB-R005.3 Refusal test row: a `dp:SecretPath` carrying a `value` (and one carrying `token`) triple fails validation, and the vocabulary file contains no property named value, secret, token, password, ciphertext or rules
- [ ] T019 [US2] MEDULLA-DATAPLAT-VOCAB-R005.4 Add a mapping-validator rule that refuses a `dp:StoreMapping` whose `dp:viaOperation` is `openbao_mcp_kv:kv2_get`, `kv1_get`, `openbao_mcp_logical:read` or `unwrap`
- [ ] T020 [US2] MEDULLA-DATAPLAT-VOCAB-R005.5 Add a shape: each `dp:PolicyPathGrant` has exactly one `dp:grantPolicy` (`dp:AccessPolicy`), exactly one `dp:grantPath` (`dp:SecretPath`), and at least one `dp:grantsCapability` string in `read list create update delete sudo deny`
- [ ] T021 [US3] MEDULLA-DATAPLAT-VOCAB-R006.1 Add the `sql` `dp:StoreMapping` fixture for `sql_schema:tables`, `columns`, `foreign_keys`, `constraints` with `dp:accelerationMode` V (reason recorded where M)
- [ ] T022 [US3] MEDULLA-DATAPLAT-VOCAB-R006.2 Add the `documentdb` `dp:StoreMapping` fixture for `documentdb_collections:list_collections`, `documentdb_analysis:aggregate` with `dp:accelerationMode` V (reason recorded where M)
- [ ] T023 [US3] MEDULLA-DATAPLAT-VOCAB-R006.3 Add the `objectstore` `dp:StoreMapping` fixture for `buckets:list`, `objects:list`, `objects:head` with `dp:accelerationMode` V (reason recorded where M)
- [ ] T024 [US3] MEDULLA-DATAPLAT-VOCAB-R006.4 Add the `vector` `dp:StoreMapping` fixture for `vector_collection_management:list_collections` with `dp:accelerationMode` V (reason recorded where M)
- [ ] T025 [US3] MEDULLA-DATAPLAT-VOCAB-R006.5 Add the `kafka` `dp:StoreMapping` fixture for `kafka_groups:lags`, `kafka_partitions:list`, `kafka_topics:list` with `dp:accelerationMode` V; M for sustained lag (reason recorded where M)
- [ ] T026 [US3] MEDULLA-DATAPLAT-VOCAB-R006.6 Add the `openbao` `dp:StoreMapping` fixture for `openbao_mcp_sys:get_mounts`, `openbao_mcp_logical:list` with `dp:accelerationMode` V (reason recorded where M)
- [ ] T027 [US3] MEDULLA-DATAPLAT-VOCAB-R006.7 Add the `clarity` `dp:StoreMapping` fixture for `clarity_insights:get_data_export` with `dp:accelerationMode` V up to 3 days; M beyond (reason recorded where M)
- [ ] T028 [US3] MEDULLA-DATAPLAT-VOCAB-R006.8 Add the `data-science` `dp:StoreMapping` fixture for `load_dataset`, `dataset_lineage`, `rank_models`, `evaluate_model` with `dp:accelerationMode` V; M for run history (reason recorded where M)
- [ ] T029 [US4] MEDULLA-DATAPLAT-VOCAB-R007.1 Add a shape rule: `materialised` without `dp:materialisationReason` is a violation; `virtual` needs none
- [ ] T030 [US4] MEDULLA-DATAPLAT-VOCAB-R007.2 Add a shape rule: `contractKind = mcp-operation` requires `dp:viaOperation`
- [ ] T031 [US4] MEDULLA-DATAPLAT-VOCAB-R007.3 Add a test that no `dp:` term or mapping fixture declares a discovery operation: relational/document entries come from `vg:MetadataContract` only
- [ ] T032 [US4] MEDULLA-DATAPLAT-VOCAB-R007.4 Add a test that every fixture mapping not marked `materialised` has no `AccelerationPolicy` and every `materialised` one cites R038
- [ ] T033 [US5] MEDULLA-DATAPLAT-VOCAB-R008.1 Add an alignment table (package term -> `dp:` term, `rdfs:subClassOf` / `owl:equivalentProperty`) for sql and documentdb
- [ ] T034 [US5] MEDULLA-DATAPLAT-VOCAB-R008.2 Add the alignment table for objectstore and vector
- [ ] T035 [US5] MEDULLA-DATAPLAT-VOCAB-R008.3 Add the alignment table for kafka, openbao, clarity, data-science
