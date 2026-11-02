//! medulla-h-compute: method handlers (compute). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "datascience")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod datascience;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "finance")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod finance;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-graph-tiles")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_tile_server;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-graph-tiles")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_tile_source;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "quantum-agent-api")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod quantum;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-static-export")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod viz;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-interactive")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod viz_interactive;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "wasm-udf")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod wasm_udf;
