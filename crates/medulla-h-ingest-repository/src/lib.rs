//! medulla-h-ingest-repository: method handlers (ingest). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "asr-whisper")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod asr;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "blob")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod blob;

/// The context read over stored knowledge atoms (MEDULLA-INGEST-WORKER-R003.3).
pub mod atom_context;

/// Chunked source drains: a chain of bounded page tasks over the one task queue
/// (MEDULLA-INGEST-WORKER-R605.2).
pub mod ingest_drain;
/// Served ingest queue entry: enqueue maps onto the served task queue with
/// deterministic ids (MEDULLA-INGEST-WORKER-R001.4).
pub mod ingest_queue;
/// Per-span embedding admission (MEDULLA-DECISION-ENGINE-R079.2, R079.3).
pub mod ingestion_admission;
/// Producing-rung recording and overwrite resistance for ingest producers
/// (MEDULLA-DECISION-ENGINE-R080.2).
pub mod ingestion_rung;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "modality-serving")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod modality;

/// The ontology slice one extraction unit needs (MEDULLA-INGEST-WORKER-R913).
#[cfg(all(feature = "blob", feature = "decide"))]
pub mod ontology_slice;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod repository_enrichment_worker;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod repository_index;

/// Lowering of the served `SemanticEnrich` writes (MEDULLA-DECISION-ENGINE-R614.3).
#[allow(missing_docs)]
pub mod semantic_enrichment;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_batch;

pub mod source_bisect;

// MEDULLA-ORCH-VOCAB-R009.101.1: the harness session index admission.
pub mod harness_session;
// MEDULLA-SCM-R008 — gate verdicts are sealed records.
/// Defect intake: a symptom lands on the defect of its signature (MEDULLA-SDLC-R003.3).
pub mod defect_intake;
pub mod gate_verdict;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_ingestion;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
