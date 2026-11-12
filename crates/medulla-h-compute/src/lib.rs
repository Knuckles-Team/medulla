//! medulla-h-compute: method handlers (compute). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

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

/// Served numeric kernel evaluation (MEDULLA-FINANCE-PRIMITIVES-R002.2.5).
#[cfg(feature = "evaluate")]
pub mod numeric;

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

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
