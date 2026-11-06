//! medulla-server-api: server state, auth, access, mutation batch API and the crate-split seam traits. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 2 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod anonymous;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod api;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod authority_context;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod batch_compile;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod carrier;

/// `ClientSession` record admission (MEDULLA-IDENTITY-R705).
pub mod client_session;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod compute;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod denial_sample;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod handlers;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod http1;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "shacl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod icv_guard;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod identity_view;

/// Typed server-config knob loader (MEDULLA-CONTRACT-R058).
pub mod knob_config;
/// Semantic model config of the embedding promotion gate (MEDULLA-UNIFIED-DATA-PLANE-R040.3).
pub mod semantic_model_config;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "server", feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_apply;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "oidc")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod oidc;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod outbox_operator;

/// The read and claim audit sink (MEDULLA-IDENTITY-R610.5).
pub mod read_audit;

/// Read scope: visible knowledge units and tenant placement (MEDULLA-IDENTITY-R023.3).
pub mod read_scope;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod request_replay;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod request_scope;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod routed_write_coalescer;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod service_grant;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod state;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod test_support;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod txn;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "wasm-udf")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod udf_catalog;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-static-export")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod viz_engine;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "viz-static-export")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod viz_provenance;

#[cfg(feature = "shacl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod schema_compose;
