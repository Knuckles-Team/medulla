//! Modules moved here from `medulla_store::redb_store` (hard cutover, no re-exports).

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_lease;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod resource;
#[allow(missing_docs)]
pub mod resource_rows;

use medulla_protocol_types::protocol::Method;
use medulla_result_types::epistemic_operations::{
    ResourceCapacitySnapshot, ResourceHostUpdateRequest, ResourceHostUpdateRequestTargetKind,
    ResourceHostUpdateResult, ResourceHostUpdateResultReason,
    ResourceHostUpdateResultSchemaVersion, ResourceHostUpdateSnapshotTargetKind,
    ResourceRequirement, ResourceReservationHostSnapshot,
    ResourceReservationHostSnapshotTargetKind, ResourceReservationRecord,
    ResourceReservationRecordState, ResourceReservationRecordTargetKind,
    ResourceReservationRequest, ResourceReservationRequestTargetKind, ResourceReservationResult,
    ResourceReservationResultDecision, ResourceReservationResultSchemaVersion,
    ResourceReservationResultState, ResourceReservationStatusRequest,
    ResourceReservationStatusResult, ResourceReservationStatusResultSchemaVersion,
    ResourceReservationSummary, ResourceReservationSummaryState, ResourceTargetSnapshot,
    ResourceTargetSnapshotKind,
};
use medulla_store::redb_store::*;
