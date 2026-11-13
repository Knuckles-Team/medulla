# MEDULLA-FEDERATED-QUERY — Data model

Governing plan: [plan.md](plan.md). The tables list the types this spec adds or extends. Each type lives in the crate that owns its layer.

## Wire and result types

| Type | File | Rows |
|---|---|---|
| `RoundRequest`, `RoundResult`, `RetrievalPremise`, `SelectionPart`, `HierarchyPart`, `ActivationBenchmark`, `InferenceCounts` | `crates/medulla-result-types/src/retrieval_round.rs` | `MEDULLA-FEDERATED-QUERY-R081.11`, `MEDULLA-FEDERATED-QUERY-R081.7.3`, `MEDULLA-FEDERATED-QUERY-R081.9.3` |
| `ChunkScorePair`, `SelectionPolicy`, `SelectionReceipt` | `crates/medulla-result-types/src/retrieval_verdict.rs` | `MEDULLA-FEDERATED-QUERY-R081.7.1` |
| `ArgumentMap`, `AifNode`, `AifEdge`, `DungProjection`, `ExtensionResult` | `crates/medulla-result-types/src/argument_map.rs` | `MEDULLA-FEDERATED-QUERY-R700.1`, `MEDULLA-FEDERATED-QUERY-R700.5` |
| `LiveArtifact`, `QuerySource`, `RefreshRecord` | `crates/medulla-source-types/src/live_artifact.rs` | `MEDULLA-FEDERATED-QUERY-R701.1` |
| `EvidenceGraph`, `SearchTask`, `RiskFinding`, `RiskReport` | `crates/medulla-work-types/src/search_task.rs` | `MEDULLA-FEDERATED-QUERY-R702.1` |
| `IntegrityShape`, `Severity`, `ShapeReport` | `crates/medulla-types/src/shape_report.rs` (new) | `MEDULLA-FEDERATED-QUERY-R005.700` |
| `SourceQueryRequest`, `ApprovedSourceQuery` | `crates/medulla-result-types/src/source_query.rs` | `MEDULLA-FEDERATED-QUERY-R085.1` |
| `SchemaContext`, `NlPlanStep` | `crates/medulla-nlq/src/nl/context.rs` | `MEDULLA-FEDERATED-QUERY-R058.4.1` |

## Graph storage

| Label or edge | Properties | Rows |
|---|---|---|
| `AifNode` node, `AIF_EDGE` edge, `ArgumentMap` node | `aif_id`, `kind`, `text`, `edge_id` | `MEDULLA-FEDERATED-QUERY-R700.3` |
| `LiveArtifact` node, `LiveArtifactRefresh` node, `REFRESHED` edge | `version`, `started_at`, `ended_at`, `row_count`, `error`, `new_version` | `MEDULLA-FEDERATED-QUERY-R701.3`, `MEDULLA-FEDERATED-QUERY-R701.5` |
| `Community` node | `summary_id`, `summary_revision`, `entity_ids` | `MEDULLA-FEDERATED-QUERY-R081.9.3` |
| `SectionNode` node | `document_id`, `document_revision`, `parent_id`, `children`, `granularity`, `source_start`, `source_end`, `summary`, `source_digest`, `summary_digest` | `MEDULLA-FEDERATED-QUERY-R081.9.7.3` |
| `RetrievalFeedback` node | the fields of `crates/medulla-retrieval/src/feedback_boost.rs` | `MEDULLA-FEDERATED-QUERY-R705.3` |

## Invariants

- Every type rejects unknown fields and non-finite numbers.
- A durable write goes through the mutation gateway as one atomic batch.
- No table is added outside the durable-table registry.
