# MEDULLA-REPO-INGEST — Data model

This spec adds typed entities for ingestion. Each entity is serde-stable and tenant-scoped.

## Entities

| Entity | Owner file | Identity | Key fields |
|---|---|---|---|
| `SkillCompilation` | `crates/medulla-ingestion-types/src/skill_plan.rs` | skill directory reference and document digest | `grammar_revision`, `plan_digest`, `capability_digest`, `type_digest`, `steps` |
| `StageReceipt` and `ThroughputWindow` | `crates/medulla-types/src/ingest_throughput.rs` | stage, paper id and revision | `attempted`, `completed`, `duplicate`, `bottleneck_stage`, `state` |
| `ReadbackReceipt` and `HydrationRun` | `crates/medulla-ingestion-types/src/ingest_readback.rs` | write receipt digest | `matched_ids`, `mismatched_ids`, `verdict`, `readback_receipt_digest` |
| `CodeMapQuery` and `CodeMapResult` | `crates/medulla-types/src/code_map.rs` | repository and source revision | `map_generation`, `inventory_digest`, `query_digest`, `next_cursor` |
| `FragmentAddress`, `Supersession`, `DeadLetterItem`, `ConformanceReport` | `crates/medulla-ingestion-types/src/ingest_capabilities.rs` | envelope id or item id | `schema_version`, `reason`, `attempts`, `fitness_milli` |
| `HydrationRunClaim` | `crates/medulla-ingestion-types/src/ingest_readback.rs` | skill-compilation node id | `workflow_revision`, `plan_digest` |
| `RepositoryFileFingerprint` node | `crates/medulla-h-ingest-repository/src/repository_index/enrich/fingerprint.rs` | `Digest256::framed(b"eg/repository-fingerprint/v1", &[repository_id, file_path])` | `content_hash`, `structure_hash` |
| `CodeHealthSweepRequest` and `CodeHealthReport` | `crates/medulla-ingestion-types/src/code_health.rs` | request only; nothing stored | `repo`, `entry_points`, `step_budget`, `findings` |
| `SectionNode`, `ProcessedDocument` (stored as `DocumentSection` and `ProcessedDocument` nodes) | `crates/medulla-document/src/processing.rs`; lowered by `crates/medulla-h-ingest/src/web_ingest.rs` | `section:{document_id}:{digest_hex[..16]}:{section_id}` and `processed-document:{document_id}:{digest_hex[..32]}` | `span_start`, `span_end`, `depth`, `coverage_milli`, `tree_check` |

## Relationships

An edge is identified by `(source_id, target_id, relation_type)`. A `SkillCompilation` node links to its skill directory node. A `DocumentSection` links to its document by `sectionOf` and to its parent by `subsectionOf`; a chunk links to its section by `inSection` and to a `DocumentLinkTarget` by `linksTo`; a document links to its `ProcessedDocument` by `processedAs`. A `supersedes` edge runs from a new fact to the fact it replaces. Every node carries its tenant and provenance.
