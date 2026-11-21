# MEDULLA-INGEST-WORKER — Data model

This file lists the entities that the ingest worker rows add. Each is a typed Rust value that the engine validates at construction; the rows hold the bounds.

## Entities

### IngestJobEnvelope

- Fields: `job_id`, `tenant`, `corpus_key`, `kind`, `target`, `attempt`, and an optional `page` for drains.
- Owner: `crates/medulla-source-types/src/ingest_job.rs`.
- Rule: a blank id or target, an unknown kind or an attempt above the retry ceiling is refused with `IngestJobError`.

### IngestRetryPolicy

- Fields: `max_attempts`, `base_delay`, `max_delay`, `jitter`.
- Relation: built by `IngestRetryPolicy::from_limits` from `SchedulerConfig.ingest_retry` (`IngestRetryLimits`, integer fields, R001.4.1) and used by `next_delay`, `on_ingest_ack` and `on_ingest_claim`.

### DeadLetterItem

- Fields (MEDULLA-REPO-INGEST-R002.650, unchanged): `item_id` (the task id), `queue`, `reason` (`INGEST_RETRIES_EXHAUSTED` or `LEASE_EXPIRED_EXHAUSTED`), `attempts`, `last_error` (the ack detail, at most 1024 bytes), `parked_at_ms`, `payload_digest`.
- Rule: one item per exhausted task, in table `TASK_DEAD_LETTERS` of `jobs.redb`, written in the same commit as the task's `Failed` transition (R001.9, R001.12). Tenant, corpus key, kind and target are read from the failed task's stored envelope by `item_id`.

### Lane, QueueClass and LaneMetrics

- Fields: a validated lane id, `Fast`, `Medium` or `SlowHeavy`, and `depth`, `oldest_age_ms`, `inflight`.
- Owner: `crates/medulla-types/src/ingest_lanes.rs`.

### DrainHandle, PageCheckpoint, GraphRoute and StageProfile

- Fields: drain id, page index and cursor, graph and route scope, stage wall time, tokens and embeddings.
- Owner: `crates/medulla-ingestion-types/src/ingest_drain.rs`.
- Rule: `route_ingest_graph` picks the first match of connector, repository, tenant, then the default graph.

### DataPrepDeclaration

- Fields: `capability`, sorted `field_refs`, `policy_digest`.
- Owner: `crates/medulla-ingestion-types/src/data_prep_declaration.rs`.
- Rule: `deny_unknown_fields`; a value with `:` or `/` is refused.

### SourceBatchKind and SourceBatchExtension

- Fields: `Records`, `Access`, `Groups`, `Enumeration`, `Containers`, `Proposals`, `Facts`, `Transcripts`, each non-record kind with one extension variant.
- Owner: `crates/medulla-source-types/src/source_ingestion.rs`.

### KnowledgeAtom, NormalizedDocument and QuarantineReceipt

- Fields: atom id, kind, text, provenance key and tenant; document id, source digest and atom ids; typed reason and replay disposition.
- Owners: `crates/medulla-ingestion-types/src/knowledge_atom.rs` and `crates/medulla-source-types/src/source_document.rs`.

### TranscriptThread and TranscriptMessage

- Fields: connector, stream, session id, message id, role, event and storage time, concept references.
- Owner: `crates/medulla-source-types/src/source_transcript.rs`.

### Fragment

- Fields: source digest, `ChunkSpan`, text and modality.
- Owner: `crates/medulla-document/src/fragment.rs`.

### OntologyProposal, ExtractedFact and CandidateClaim

- Fields: classes, properties, `Draft` or `Admitted` state; subject, predicate, object, evidence span and confidence.
- Owners: `crates/medulla-ingestion-types/src/ontology_proposal.rs` and `crates/medulla-ingestion-types/src/extracted_fact.rs`.

### RecordGovernance, AccessSnapshot, SourceGroup, SourceEnumeration, ReconcilePlan and SourceContainer

- Owner: `crates/medulla-ingestion-types/src/source_governance.rs`.
- Rule: classification is a closed set; role and marking sets hold at most 256 entries.

### IngestBatchReport and ClassReport

- Fields: `written`, `unchanged`, `refused` counters, and one report per ontology class.
- Owner: `crates/medulla-types/src/ingest_materialize.rs`.
