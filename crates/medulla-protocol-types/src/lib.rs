//! medulla-protocol-types: Method registry and wire-query envelope types shared by every listener and client.
//!
//! Level L2 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: public-engine-contract-and-release (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-REPO-INGEST-R700 — code-health findings from a bounded reachability sweep.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod change_envelope;
// MEDULLA-UNIFIED-DATA-PLANE-R021.1 — typed Debezium Kafka change-event envelope
// shape and its op-code/before-after refusal rules. Pure data; the Kafka
// consumer and the real ChangeEnvelope conversion are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod debezium_bridge;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod messaging_wire;
// RF-RULING-004 — untrusted serialized mutation request/evidence DTOs only.
// Executable admitted plans/tokens are private to the future medulla-transaction kernel.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod error_registry;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_batch;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod protocol;
// F3: the typed result contract -- one marker per method result, compile-checked at every
// handler site and walked by `medulla-capabilities` to publish the result schemas.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod protocol_fixtures;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod result_contract;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod storage_wire;

/// The six native capacity writes. Keep admission, identity normalization and
/// served dispatch on the same variant set.
#[macro_export]
macro_rules! capacity_lease_writes {
    () => {
        $crate::protocol::Method::AcquireCapacity { .. }
            | $crate::protocol::Method::RenewCapacity { .. }
            | $crate::protocol::Method::ReleaseCapacity { .. }
            | $crate::protocol::Method::ReclaimExpiredCapacity { .. }
            | $crate::protocol::Method::UpdateCapacityCell { .. }
            | $crate::protocol::Method::ThrottleCapacityCell { .. }
    };
}
