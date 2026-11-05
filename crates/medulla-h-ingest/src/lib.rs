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
