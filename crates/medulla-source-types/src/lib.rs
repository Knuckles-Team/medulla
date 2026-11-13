//! medulla-source-types: Source ingestion, source documents and transcripts, ingest jobs and development-lifecycle record types (changeset review, pre-merge tests, standards drift, live artifacts, supply-chain reports).
//!
//! Layer L1d (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-INGEST-WORKER-R001.1/.2/.3/.7/.8 — ingest job envelope, lanes, partition key, retry.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ingest_job;
// MEDULLA-INGEST-WORKER-R003.7 — normalized documents and accepted/quarantined outcomes.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_document;
// MEDULLA-INGEST-WORKER-R003.13 — source transcript threads and messages.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_transcript;
// MEDULLA-DURABLE-KERNEL-R087.1 — typed semantic changeset-review model. The
// authenticated producer/consumer route is a later R087.2 concern.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod changeset_review;
// MEDULLA-UNIFIED-DATA-PLANE-R701 — standards, cohorts, drift and consolidation.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod standards_drift;
// MEDULLA-CQ-XSRC-R006.1/R006.4 — the typed supply-chain impact report (one
// report: impact per service, the items behind each, the sources used and
// why, and typed abstentions) and its pure, deterministic assembly from the
// planned query's rows and premises.
pub mod source_sync;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod supply_chain_report;
// MEDULLA-CQ-XSRC-R008.4 — the typed report of the three cross-source questions.
#[allow(missing_docs)]
pub mod cross_source_report;
// MEDULLA-DURABLE-KERNEL-R011 — the required pre-merge test-module category
// (raft/backup/persistence) and a validated declaration within one.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pre_merge_test_module;
// MEDULLA-SDLC — release, deployment, defect, derived status and lifecycle trace records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lifecycle;
// MEDULLA-SDLC-R030.1 — the defect machine: edge table D01 to D09 and its one transition function.
pub mod defect_machine;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_ingestion;
// MEDULLA-FEDERATED-QUERY-R701 — live artifacts re-derived from a query source.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod live_artifact;
// MEDULLA-FEDERATED-QUERY-R708 — stored, versioned queries and their materialized results.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stored_query;
