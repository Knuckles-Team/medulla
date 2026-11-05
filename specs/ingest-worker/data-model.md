# MEDULLA-INGEST-WORKER — Data model

This file lists the entities that the ingest worker rows add. Each is a typed Rust value that the engine validates at construction; the rows hold the bounds.

## Entities

### IngestJobEnvelope

- Fields: `job_id`, `tenant`, `corpus_key`, `kind`, `target`, `attempt`, and an optional `page` for drains.
- Owner: `crates/medulla-source-types/src/ingest_job.rs`.
- Rule: a blank id or target, an unknown kind or an attempt above the retry ceiling is refused with `IngestJobError`.

### IngestRetryPolicy

- Fields: `max_attempts`, `base_delay`, `max_delay`, `jitter`.
- Relation: read from `SchedulerConfig.ingest_retry` and used by `next_delay`.

### DeadLetterItem

- Fields: queue, task id, tenant, corpus key, kind, target, attempts, `last_error_code`, `last_error_digest`.
- Rule: one item per exhausted task, written by `on_ingest_ack` through `park`; the error text is cut to 1 KiB.

### Lane, QueueClass and LaneMetrics

- Fields: a validated lane id, `Fast`, `Medium` or `SlowHeavy`, and `depth`, `oldest_age_ms`, `inflight`.
- Owner: `crates/medulla-types/src/ingest_lanes.rs`.

### DrainHandle, PageCheckpoint, GraphRoute and StageProfile

- Fields: drain id, page index and cursor, graph and route scope, stage wall time, tokens and embeddings.
- Owner: `crates/medulla-types/src/ingest_drain.rs`.
- Rule: `route_ingest_graph` picks the first match of connector, repository, tenant, then the default graph.

### DataPrepDeclaration

- Fields: `capability`, sorted `field_refs`, `policy_digest`.
- Owner: `crates/medulla-types/src/data_prep_declaration.rs`.
- Rule: `deny_unknown_fields`; a value with `:` or `/` is refused.

### SourceBatchKind and SourceBatchExtension

- Fields: `Records`, `Access`, `Groups`, `Enumeration`, `Containers`, `Proposals`, `Facts`, `Transcripts`, each non-record kind with one extension variant.
- Owner: `crates/medulla-source-types/src/source_ingestion.rs`.

### KnowledgeAtom, NormalizedDocument and QuarantineReceipt

- Fields: atom id, kind, text, provenance key and tenant; document id, source digest and atom ids; typed reason and replay disposition.
- Owners: `crates/medulla-types/src/knowledge_atom.rs` and `crates/medulla-source-types/src/source_document.rs`.

### TranscriptThread and TranscriptMessage

- Fields: connector, stream, session id, message id, role, event and storage time, concept references.
- Owner: `crates/medulla-source-types/src/source_transcript.rs`.

### Fragment

- Fields: source digest, `ChunkSpan`, text and modality.
- Owner: `crates/medulla-document/src/fragment.rs`.

### OntologyProposal, ExtractedFact and CandidateClaim

- Fields: classes, properties, `Draft` or `Admitted` state; subject, predicate, object, evidence span and confidence.
- Owners: `crates/medulla-types/src/ontology_proposal.rs` and `crates/medulla-types/src/extracted_fact.rs`.

### RecordGovernance, AccessSnapshot, SourceGroup, SourceEnumeration, ReconcilePlan and SourceContainer

- Owner: `crates/medulla-types/src/source_governance.rs`.
- Rule: classification is a closed set; role and marking sets hold at most 256 entries.

### IngestBatchReport and ClassReport

- Fields: `written`, `unchanged`, `refused` counters, and one report per ontology class.
- Owner: `crates/medulla-types/src/ingest_materialize.rs`.
