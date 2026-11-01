//! medulla-h-ingest: method handlers (ingest). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

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

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "query", feature = "modality-serving"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod web_ingest;
