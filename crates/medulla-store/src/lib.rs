//! medulla-store: redb store, layout and sidecar. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 1 in `scripts/crate_split/manifest.toml`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "statechart")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod loop_statechart;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_layout;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_store;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sidecar_store;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "statechart")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_statechart;
