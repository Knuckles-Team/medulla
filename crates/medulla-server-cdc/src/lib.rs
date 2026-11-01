//! medulla-server-cdc: Change data capture hub, CEP, materialised views, reasoning cascade and impact watch.
//!
//! Level F4b in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "result-cache", feature = "streaming"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cache_coherence;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "streaming")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cdc;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "streaming", feature = "stream"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cep;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "streaming", feature = "mining", feature = "query"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod impact_watch;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "matview")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod matview;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "owl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning_cascade;
