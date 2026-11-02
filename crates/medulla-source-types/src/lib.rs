//! medulla-source-types: Source ingestion, source documents and transcripts, ingest jobs and development-lifecycle record types (changeset review, pre-merge tests, standards drift, live artifacts, supply-chain reports).
//!
//! Level L1d in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod supply_chain_report;
// MEDULLA-DURABLE-KERNEL-R011 — the required pre-merge test-module category
// (raft/backup/persistence) and a validated declaration within one.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pre_merge_test_module;
// MEDULLA-SDLC — release, deployment, defect, derived status and lifecycle trace records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lifecycle;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_ingestion;
// MEDULLA-FEDERATED-QUERY-R701 — live artifacts re-derived from a query source.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod live_artifact;
