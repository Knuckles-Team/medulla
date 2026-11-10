//! medulla-h-ingest: method handlers (ingest). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
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

/// Served ingest queue entry: enqueue maps onto the served task queue with
/// deterministic ids (MEDULLA-INGEST-WORKER-R001.4).
pub mod ingest_queue;
/// Producing-rung recording and overwrite resistance for ingest producers
/// (MEDULLA-DECISION-ENGINE-R080.2).
pub mod ingestion_rung;
/// Dataset lineage ingest (MEDULLA-UNIFIED-DATA-PLANE-R702.3).
pub mod lineage;
/// Served meeting transcript revisions and summaries
/// (MEDULLA-MEDIA-VOCAB-R001.101.3 / .4 / .5).
#[allow(missing_docs)] // fields mirror the medulla-types wire contracts
pub mod meeting_transcript;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "modality-serving")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod modality;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod repository_enrichment_worker;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod repository_index;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_batch;

/// Blob phase and path-set acceptance of source-changeset intake
/// (MEDULLA-SCM-R001, MEDULLA-SCM-R003.1).
#[cfg(feature = "blob")]
pub mod source_changeset;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_ingestion;

/// Lowering of a planned Document delete cascade (MEDULLA-UNIFIED-DATA-PLANE-R651.3).
#[cfg(all(feature = "query", feature = "modality-serving"))]
#[allow(missing_docs)]
pub mod document_delete;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "query", feature = "modality-serving"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod web_ingest;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
