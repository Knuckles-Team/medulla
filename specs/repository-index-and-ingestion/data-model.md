# MEDULLA-REPO-INGEST — Data model

This spec adds typed entities for ingestion. Each entity is serde-stable and tenant-scoped.

## Entities

| Entity | Owner file | Identity | Key fields |
|---|---|---|---|
| `SkillCompilation` | `crates/medulla-types/src/skill_plan.rs` | skill directory reference and document digest | `grammar_revision`, `plan_digest`, `capability_digest`, `type_digest`, `steps` |
| `StageReceipt` and `ThroughputWindow` | `crates/medulla-types/src/ingest_throughput.rs` | stage, paper id and revision | `attempted`, `completed`, `duplicate`, `bottleneck_stage`, `state` |
| `ReadbackReceipt` and `HydrationRun` | `crates/medulla-types/src/ingest_readback.rs` | write receipt digest | `matched_ids`, `mismatched_ids`, `verdict`, `readback_receipt_digest` |
| `CodeMapQuery` and `CodeMapResult` | `crates/medulla-types/src/code_map.rs` | repository and source revision | `map_generation`, `inventory_digest`, `query_digest`, `next_cursor` |
| `FragmentAddress`, `Supersession`, `DeadLetterItem`, `ConformanceReport` | `crates/medulla-types/src/ingest_capabilities.rs` | envelope id or item id | `schema_version`, `reason`, `attempts`, `fitness_milli` |
| `CdcEnvelopeMap`, `EventLogProjection`, `OcelDocument`, `ReferenceLink` | `crates/medulla-durable/src/adapters.rs` | source and log id | `operation_field`, `case_object_type`, `projection_hash` |
| `SectionNode`, `ProcessedDocument` | `crates/medulla-document/src/processing.rs` | document id and section id | `span_start`, `span_end`, `coverage_milli`, `tree_check` |

## Relationships

An edge is identified by `(source_id, target_id, relation_type)`. A `SkillCompilation` node links to its skill directory node. A `ProcessedDocument` node links to its `Section` nodes. Every node carries its tenant and provenance.
