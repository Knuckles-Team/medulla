# MEDULLA-DATAPLAT-VOCAB — Data model

This spec adds one ontology module, `core:data-platform@1`, and its shapes. Prefix `dp:` is `https://knuckles-team.github.io/ontology/data-platform#`. Terms live in `crates/medulla-core/ontology/data_platform-v1.ttl`; shapes live in `crates/medulla-core/ontology/data_platform-v1.shapes.ttl`.

## Entities

### Shared and mapping terms

- Classes: `dp:DataStore`, `dp:StoreMapping` (subclass of `vg:VirtualMapping`).
- Properties: `dp:storeFamily`, `dp:inStore`, `dp:viaConnection`, `dp:asOf`, `dp:contractKind`, `dp:viaOperation`, `dp:accelerationMode`, `dp:materialisationReason`.
- Rule: `dp:accelerationMode` is exactly one of `virtual`, `materialised`; a `materialised` mapping carries `dp:materialisationReason`; `dp:contractKind` is one of `r2rml-sql`, `document-shape`, `mcp-operation`.

### Relational and document terms

- Classes: `dp:Database`, `dp:Schema`, `dp:Table`, `dp:View`, `dp:Column`, `dp:ForeignKey`, `dp:Collection`, `dp:DocumentField`.
- Properties: `dp:inSchema`, `dp:hasColumn`, `dp:referencesTable`, `dp:isPrimaryKey`, `dp:sqlDialect`, `dp:fieldPath`, `dp:fieldType`, `dp:presenceRatio`, `dp:documentCount`.
- Rule: `dp:presenceRatio` is in [0,1]; `dp:fieldType` is in the `DocumentFieldType` list including `Mixed`.

### Object, vector, stream, analytics and ML terms

- Classes: `dp:Bucket`, `dp:StoredObject`, `dp:VectorCollection`, `dp:EmbeddingSpace`, `dp:Topic`, `dp:Partition`, `dp:ConsumerGroup`, `dp:PartitionLag`, `dp:AnalyticsSite`, `dp:BehaviorMetric`, `dp:Dataset` (subclass of the core `:Dataset`), `dp:Model`, `dp:TrainingRun`, `dp:EvaluationResult`.
- Properties: `dp:objectKey`, `dp:byteSize`, `dp:lastModified`, `dp:embeddingModel`, `dp:dimensions`, `dp:distanceMetric`, `dp:usesEmbeddingSpace`, `dp:vectorCount`, `dp:partitionOf`, `dp:subscribesTo`, `dp:lag`, `dp:groupState`, `dp:metricName`, `dp:metricValue`, `dp:windowDays`, `dp:dimensionName`, `dp:dimensionValue`, `dp:trainedOn`, `dp:producedModel`, `dp:evaluatedOn`, `dp:baselineRun`.
- Rule: `dp:StoredObject` has no content property; `dp:dimensions` is a positive integer; `dp:lag` is an integer at least 0 and `dp:PartitionLag` requires `dp:asOf`; `dp:Model` is not equivalent to the core LLM or embedding `:Model`.

### Secret metadata terms

- Classes: `dp:SecretEngine`, `dp:SecretPath`, `dp:AccessPolicy`, `dp:RotationState`, `dp:PolicyPathGrant`.
- Properties: `dp:hasRotationState`, `dp:lastRotatedAt`, `dp:currentVersion`, `dp:pathName`, `dp:grantPolicy`, `dp:grantPath`, `dp:grantsCapability`, `dp:leaseExpiresAt`.
- Rule: each of the five classes has a closed shape; no property named value, secret, token, password, ciphertext or rules exists; `dp:grantsCapability` is one of `read list create update delete sudo deny`.

## Proposed terms (owner review)

None. Every term above is declared in `data_platform-v1.ttl`.
