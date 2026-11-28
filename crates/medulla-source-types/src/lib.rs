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
// MEDULLA-DECISION-ENGINE-R869.2 — the typed items of an `action_records` batch.
pub mod source_action_record;
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
// MEDULLA-SCM-R002 — the source changeset record and its structured trailers.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_changeset;
// MEDULLA-SCM-R007 — path-scoped access grants of the source methods.
pub mod source_path_grant;
// MEDULLA-SDLC — release, deployment, defect, derived status and lifecycle trace records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lifecycle;
// MEDULLA-SCM-R008 — gate verdict records and the landing policy.
pub mod gate_verdict;
// MEDULLA-SDLC-R002.7 — the durable record of one applied deployment plan.
pub mod deploy_record;
// MEDULLA-BUSINESS-OPS-R004 — the approval gate question answered from the graph's approval rules.
pub mod approval_gate;
// MEDULLA-DECISION-ENGINE-R901 — agent goals, their iterations and the goal state machine.
pub mod agent_goal;
// MEDULLA-UNIFIED-DATA-PLANE-R790 — connector source-sync runs and the run list.
pub mod connector_run;
// MEDULLA-UNIFIED-DATA-PLANE-R791 — source connections and their approved field mappings.
pub mod source_connection;
// MEDULLA-SCM-R009 — the bisect request and answer over the gate verdicts of a chain.
pub mod source_bisect;
// MEDULLA-IDENTITY-R740 — the layered configuration read.
pub mod config_layers;
pub mod spec_question;
pub mod stack_frame;
// MEDULLA-SDLC-R019.2 — delivery records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod delivery;
// MEDULLA-SDLC-R030.1 — the defect machine: edge table D01 to D09 and its one transition function.
pub mod defect_machine;
// MEDULLA-SDLC-R030.4 — the requirement-row state machine.
pub mod spec_machine;
// MEDULLA-SDLC-R030.5 — the task state machine.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_ingestion;
// MEDULLA-UNIFIED-DATA-PLANE-R038.7.1 — the web metasearch capability and its result record.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_symbol_edit;
pub mod task_machine;
// MEDULLA-FEDERATED-QUERY-R701 — live artifacts re-derived from a query source.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod live_artifact;
// MEDULLA-MEDIA-VOCAB-R001.101.1 — meeting transcript revisions and summary
// contracts. Pure serde.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod meeting_transcript;
// MEDULLA-FEDERATED-QUERY-R708 — stored, versioned queries and their materialized results.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod stored_query;
// MEDULLA-BUSINESS-OPS-R003.1: the owner digest as stored queries.
// MEDULLA-TYPED-PACKS-R806 - the served schema drift records and repair approval.
pub mod business_digest;
pub mod catalog_queries;
pub mod schema_drift_methods;
// MEDULLA-MACHINE — canonical machine identity, lifecycle machine, hosts file and inventory records.
pub mod machine;
