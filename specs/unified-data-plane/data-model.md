# MEDULLA-UNIFIED-DATA-PLANE — Data model

This file names the entities that the rows of `requirements.md` add or reuse. Each entity has one owning file and one rule.

## Entities

### Source record

- Fields: `source_id`, `tenant_id`, `owner_id`, `endpoint_binding`, `secret_ref`, `dialect`, `dialect_version`, `capabilities`, `allowed_catalog_scope`, `capture_config`, `current_catalog_version`, `status`; optional acceleration, quota, write-back and `read_role` policies.
- Owner: `crates/medulla-server-ops/src/api/query_txn/foreign_catalog.rs` and `ForeignSourceSpec` in `crates/medulla-plan`.
- Rule: the record holds a secret reference and never a secret value; the target is verified before the record persists.

### AttachedCatalogGraph

- Fields: tables, columns, keys, foreign keys, indexes, views, enums and a content hash.
- Owner: `crates/medulla-catalog-types/src/attached_catalog.rs`.
- Rule: the hash is stable under reordering, and a duplicate table name refuses.

### MetadataContract and AccelerationPolicy

- Fields: schemas, entities, relationships, endpoints, schema version and content digest; a copy policy with a hot-subset predicate, a row bound and a watermark.
- Owner: `crates/medulla-types/src/metadata_acceleration.rs`.
- Rule: a policy for a source that cannot capture refuses, and a missing watermark falls back to a live read.

### VirtualMapping

- Fields: ontology class, discovered entity, key field, predicate-to-field pairs, approval state.
- Owner: the `core:virtual-graph@1` vocabulary and shapes under `crates/medulla-core/ontology/`.
- Rule: only an approved mapping serves reads, and a mapping that names an undiscovered entity or field is refused.

### SourceCategory and SnapshotSemantics

- Fields: a closed category list; isolation (`Snapshot`, `RepeatableRead`), snapshot token and replica lag bound.
- Owner: `crates/medulla-types/src/metadata_acceleration.rs`.
- Rule: serde rejects an unknown category or isolation, and the default lag bound is 5000 ms.

### ChangeEnvelope and ConsumerCheckpoint

- Fields: source position, transaction boundary, table or object ID, operation, before and after shape, schema version, event ID; consumer name and applied position.
- Owner: `crates/medulla-server-cdc` and `crates/medulla-types`.
- Rule: a replay applies an incoming position once, and an unnamed consumer refuses.

### ModalityCheck and PromotionVerdict

- Fields: modality (`text`, `image`, `audio`), probe set digest, metric, floor, baseline; verdict per modality.
- Owner: `crates/medulla-types/src/embedding_promotion.rs`.
- Rule: one regression blocks promotion, and the verdict is a graph node linked to the model and version.

### IncidentRecord and AnomalyReading

- Fields: entity, signature digest, window, evidence set, status order.
- Owner: `crates/medulla-stream`.
- Rule: one open incident per signature, with a closed status order.
