//! medulla-store: redb store, layout and sidecar. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 1 in `scripts/crate_split/manifest.toml`.
#![recursion_limit = "256"]

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// The native WorkItem-kernel lease and lifecycle writes (claim, renew,
/// cancel, defer, metadata CAS, the graph-os MEDULLA-2 control-lease writes and the
/// EH-558 sealed-record retirement),
/// as ONE pattern. The apply and classification sites match through it, so a
/// new family member is added once and every exhaustive `match` using it
/// still fails to compile until it is classified. The dispatch arm in
/// `medulla_server::handlers::work_item` stays enumerated: the contract-reachability
/// gate reads dispatch arms by variant name.
#[macro_export]
macro_rules! work_item_kernel_writes {
    () => {
        ::medulla_protocol_types::protocol::Method::ClaimWorkItem { .. }
            | ::medulla_protocol_types::protocol::Method::RenewWorkItemLease { .. }
            | ::medulla_protocol_types::protocol::Method::CancelWorkItem { .. }
            | ::medulla_protocol_types::protocol::Method::DeferWorkItem { .. }
            | ::medulla_protocol_types::protocol::Method::CasWorkItemMetadata { .. }
            | ::medulla_protocol_types::protocol::Method::PublishCurrentPlan { .. }
            | ::medulla_protocol_types::protocol::Method::IssueHostStartGrant { .. }
            | ::medulla_protocol_types::protocol::Method::PublishRunCheckpoint { .. }
            | ::medulla_protocol_types::protocol::Method::IssueControlLease { .. }
            | ::medulla_protocol_types::protocol::Method::TransitionControlLease { .. }
            | ::medulla_protocol_types::protocol::Method::PolicyEvolutionStore { .. }
            | ::medulla_protocol_types::protocol::Method::GapUpsert { .. }
            | ::medulla_protocol_types::protocol::Method::GapTransition { .. }
            | ::medulla_protocol_types::protocol::Method::GapSettle { .. }
            | ::medulla_protocol_types::protocol::Method::WorkOfferPut { .. }
            | ::medulla_protocol_types::protocol::Method::RetireSealedRecord { .. }
    };
}

/// The WorkItem resource-reservation writes, as one pattern (see
/// `work_item_kernel_writes`).
#[macro_export]
macro_rules! work_item_resource_writes {
    () => {
        ::medulla_protocol_types::protocol::Method::ReserveWorkItemResources { .. }
            | ::medulla_protocol_types::protocol::Method::ReleaseWorkItemResources { .. }
            | ::medulla_protocol_types::protocol::Method::ReclaimWorkItemResources { .. }
            | ::medulla_protocol_types::protocol::Method::UpdateResourceHost { .. }
    };
}

/// One staged time-series measurement batch for a cross-modal ACID commit
/// (CONCEPT:MEDULLA-KG.backend.cross-modal-atomic-commit): `(series_id, n_fields, bucket_ns, field_names, points)` where each
/// point is `(ts_nanos, field_values)`. Carried as PLAIN data (no `medulla-tsdb` type) so it
/// threads through the `PersistenceBackend` trait + the server-independent `redb_store`
/// without pulling the `tsdb` feature into either — the redb backend converts it to
/// `medulla_tsdb::point::Point` only where the SERIES tables are actually written. Kept as a
/// named alias so the `&[MeasurementBatch]` params don't trip the `type_complexity` lint.
pub type MeasurementBatch = (String, usize, u64, Vec<String>, Vec<(i64, Vec<f64>)>);

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
