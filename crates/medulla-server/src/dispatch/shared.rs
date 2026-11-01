//! Helpers shared by the dispatch modules: graph residency, graph read authority, typed responses and the request limits.

use std::ops::ControlFlow;
use std::sync::Arc;
#[cfg(feature = "ast")]
use std::sync::OnceLock;
use std::time::Instant as DispatchLockInstant;
use tokio::sync::RwLock;
use tracing::info;

use medulla_server_auth::access::check_caller_is_known;
use medulla_server_auth::access::check_graph_access;
use medulla_server_auth::access::is_admin_authz_action;
use medulla_server_auth::access::require_admin_capability;
use medulla_server_auth::access::requires_write;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_auth::auth::verify_multisig_mutation_signatures;
use medulla_server_auth::auth::verify_register_identity_signature;
use medulla_server_api::authority_context::VerifiedRequestContext;
#[cfg(feature = "ast")]
use medulla_server_api::compute::compute_off_lock;
// NOT `#[cfg(feature = "redb")]`: `server::persistence` is an unconditional module
// (server/mod.rs) and three ungated signatures below -- `reconcile_existing_graph_create`,
// `read_committed_graph_version`, `reconcile_missing_graph_delete` -- name this trait
// unqualified. Gating the import broke every build without `redb`
// (`--no-default-features --features server`) with E0405 while the default full build,
// which enables `redb`, stayed green. See plans/complex/lane-reports/WD10-R-DISPATCH.md.
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Request;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::{timed_write, ServerState};

/// Stable, privacy-safe rejection for a graph lifecycle type that this binary
/// does not support.  The wire decoder normally rejects unknown enum strings
/// before dispatch; keeping this code at the authenticated dispatch boundary
/// is defense in depth for in-process callers and future enum extensions.
pub const UNSUPPORTED_GRAPH_TYPE: &str = "INVALID_ARGUMENT: unsupported graph type";

/// The identity registry is stored on the control graph, not on the caller's
/// selected data graph.  Keep this boundary exact: accepting an empty or
/// alternate graph here would make the signed request's graph claim
/// meaningless for an identity read-back.
pub const IDENTITY_GRAPH: &str = "__commons__";
pub const IDENTITY_GRAPH_SCOPE_ERROR: &str =
    "INVALID_ARGUMENT: GetIdentity requires the __commons__ graph";

/// Resolve one graph and its caller-bound row authority under the same state view.
/// Open a catalog-known but cold graph before a direct registry read.
///
/// Readers that look a graph up in the resident registry (rather than
/// routing a graph operation, which lazy-opens) would otherwise refuse a
/// graph that is merely cold, as `__commons__` is after a restart.
pub async fn ensure_graph_resident(state: &Arc<RwLock<ServerState>>, graph: &str) {
    #[cfg(feature = "redb")]
    if timed_read(state).await.registry.get(graph).is_none() {
        medulla_persistence::cold_offload::lazy_open(
            state,
            graph,
            medulla_persistence::cold_offload::max_resident_graphs(),
            medulla_persistence::cold_offload::lazy_open_page_size(),
        )
        .await;
    }
    #[cfg(not(feature = "redb"))]
    let _ = (state, graph);
}

pub fn authorized_graph_read(
    state: &ServerState,
    graph: &str,
    verified: &VerifiedRequestContext,
    missing_error: &str,
) -> Result<(Arc<medulla_graph_core::graph::GraphCore>, GraphReadAuthority), String> {
    let entry = state.registry.get(graph).ok_or(missing_error)?;
    check_graph_access(
        &state.isolation,
        Some(verified.agent_id()),
        graph,
        entry.graph_type,
        entry.owner.as_deref(),
        AccessLevel::Read,
    )?;
    let authority = GraphReadAuthority::from_verified(verified, &state.isolation)?;
    Ok((entry.core.clone(), authority))
}

/// Encode a typed method result while preserving both operation and codec errors.
pub fn typed_response<M: medulla_protocol_types::result_contract::MethodResult>(
    req_id: u64,
    result: Result<M::Body, String>,
) -> Response {
    match result.and_then(ResultPayload::of::<M>) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

pub fn validate_get_identity_graph(request_graph: &str) -> Result<(), &'static str> {
    if request_graph == IDENTITY_GRAPH {
        Ok(())
    } else {
        Err(IDENTITY_GRAPH_SCOPE_ERROR)
    }
}

#[cfg(test)]
mod identity_graph_scope_tests {
    use super::{validate_get_identity_graph, IDENTITY_GRAPH_SCOPE_ERROR};

    #[test]
    fn accepts_only_the_identity_registry_graph() {
        assert!(validate_get_identity_graph("__commons__").is_ok());
        for graph in ["", "agent:planner", "__commons__ ", "__COMMONS__"] {
            assert_eq!(
                validate_get_identity_graph(graph),
                Err(IDENTITY_GRAPH_SCOPE_ERROR)
            );
        }
    }

    #[test]
    fn scope_error_is_typed_and_does_not_echo_request_graph() {
        assert!(IDENTITY_GRAPH_SCOPE_ERROR.starts_with("INVALID_ARGUMENT:"));
        assert!(!IDENTITY_GRAPH_SCOPE_ERROR.contains("agent:planner"));
    }
}

/// Validate the graph lifecycle type before any placement, persistence, or
/// registry work begins.  `GraphType` is intentionally a closed wire enum
/// today, but the explicit allowlist means a future enum variant cannot be
/// accepted by this boundary accidentally until its lifecycle contract is
/// deliberately reviewed.
pub fn validate_graph_create_type(graph_type: medulla_protocol_types::protocol::GraphType) -> Result<(), &'static str> {
    if matches!(
        graph_type,
        medulla_protocol_types::protocol::GraphType::Agent
            | medulla_protocol_types::protocol::GraphType::Team
            | medulla_protocol_types::protocol::GraphType::Global
            | medulla_protocol_types::protocol::GraphType::Commons
    ) {
        Ok(())
    } else {
        Err(UNSUPPORTED_GRAPH_TYPE)
    }
}

#[cfg(test)]
mod graph_create_type_validation_tests {
    use super::{validate_graph_create_type, UNSUPPORTED_GRAPH_TYPE};
    use medulla_protocol_types::protocol::GraphType;

    #[test]
    fn accepts_every_supported_graph_lifecycle_type() {
        for graph_type in [
            GraphType::Agent,
            GraphType::Team,
            GraphType::Global,
            GraphType::Commons,
        ] {
            assert!(validate_graph_create_type(graph_type).is_ok());
        }
    }

    #[test]
    fn unsupported_type_error_is_stable_and_secret_free() {
        assert_eq!(
            UNSUPPORTED_GRAPH_TYPE,
            "INVALID_ARGUMENT: unsupported graph type"
        );
        assert!(!UNSUPPORTED_GRAPH_TYPE.contains("secret"));
    }
}

// ── D-EIMG-2: the process-wide dispatch lock, instrumented at its chokepoint ──
//
// Every dispatched method acquires this one `Arc<RwLock<ServerState>>` before it can do
// anything. It is the single global serialization point in the engine, and it was
// completely uninstrumented: `medulla_write_lock_wait_seconds` covers only the
// PER-GRAPH topology lock inside the write coalescer, so nothing measured the wait here.
//
// These two helpers are the ONLY sanctioned way to take that lock inside this module, so
// that instrumentation cannot be bypassed by a future call site the way it would be if
// each of the ~50 acquisitions timed itself inline (and inline timing would also rot the
// moment someone adds acquisition 51). The timing window is exactly enqueue → guard
// acquired; the guard is returned unchanged, so borrow lifetimes at every call site are
// identical to a bare `timed_read(state).await`.
//
// Cost on the fast path is one `Instant::now()` pair per acquisition — the same
// instrumentation the write coalescer already pays per batch.

/// Acquire the process-wide `ServerState` read lock, recording the wait (D-EIMG-2).
pub async fn timed_read(
    state: &Arc<RwLock<ServerState>>,
) -> tokio::sync::RwLockReadGuard<'_, ServerState> {
    let started = DispatchLockInstant::now();
    let guard = state.read().await;
    medulla_kernel::metrics::observe_dispatch_lock_wait("read", started.elapsed().as_secs_f64());
    guard
}

#[cfg(test)]
mod resource_status_privacy_tests {
    use super::*;
    use medulla_result_types::epistemic_operations::ResourceReservationHostCapacitySnapshot;
    use medulla_result_types::epistemic_operations::ResourceReservationHostSnapshot;
    use medulla_result_types::epistemic_operations::ResourceReservationHostSnapshotTargetKind;
    use medulla_result_types::epistemic_operations::ResourceReservationStatusRequest;
    use medulla_result_types::epistemic_operations::ResourceReservationStatusRequestSchemaVersion;
    use medulla_result_types::epistemic_operations::ResourceReservationStatusResult;
    use medulla_result_types::epistemic_operations::ResourceReservationStatusResultSchemaVersion;
    use medulla_result_types::epistemic_operations::ResourceReservationSummary;
    use medulla_result_types::epistemic_operations::ResourceReservationSummaryState;

    fn request(host_ref: &str) -> ResourceReservationStatusRequest {
        ResourceReservationStatusRequest {
            schema_version: ResourceReservationStatusRequestSchemaVersion::V1,
            tenant_ref: "tenant-a".to_string(),
            work_item_id: None,
            reservation_id: None,
            host_ref: Some(host_ref.to_string()),
            owner_id: None,
            fence: None,
            attempt: None,
            lease_epoch: None,
            fencing_token: None,
            input_fingerprint: None,
            fairness_group: None,
            limit: 10,
            cursor: None,
            now_ms: 10,
        }
    }

    fn result(summary_host: &str) -> ResourceReservationStatusResult {
        ResourceReservationStatusResult {
            schema_version: ResourceReservationStatusResultSchemaVersion::V1,
            complete: true,
            next_cursor: None,
            host_snapshot: Some(ResourceReservationHostSnapshot {
                host_ref: "host-secret".to_string(),
                revision: 4,
                capacity: ResourceReservationHostCapacitySnapshot {
                    cpu_weight: 8,
                    memory_mib: 8_192,
                    disk_mib: 10_000,
                    process_slots: 4,
                },
                observed: ResourceReservationHostCapacitySnapshot {
                    cpu_weight: 1,
                    memory_mib: 1_024,
                    disk_mib: 100,
                    process_slots: 1,
                },
                heartbeat_at_ms: 9,
                heartbeat_ttl_ms: 120_000,
                draining: false,
                quarantined: false,
                labels: vec!["private".to_string()],
                target_kind: ResourceReservationHostSnapshotTargetKind::Local,
                target_alias: None,
                disk_used_mib: 100,
                disk_capacity_mib: 10_000,
                held_cpu_weight: 7,
                held_memory_mib: 700,
                held_disk_mib: 70,
                held_process_slots: 1,
                disk_policies: Vec::new(),
            }),
            host_ref: Some("host-secret".to_string()),
            host_revision: 4,
            held_cpu_weight: 7,
            held_memory_mib: 700,
            held_disk_mib: 70,
            held_process_slots: 1,
            fairness_debt: 7,
            reservations: vec![ResourceReservationSummary {
                reservation_id: "reservation-1".to_string(),
                work_item_id: "work-1".to_string(),
                attempt: 1,
                host_ref: summary_host.to_string(),
                profile_name: "light-check".to_string(),
                fairness_group: "default".to_string(),
                state: ResourceReservationSummaryState::Reserved,
                revision: 1,
                expires_at_ms: 100,
                held_cpu_weight: 2,
                held_memory_mib: 200,
                held_disk_mib: 20,
                held_process_slots: 1,
                tombstone: false,
            }],
            orphan_count: 0,
            superseded_count: 0,
        }
    }

    fn decode(payload: ResultPayload) -> ResourceReservationStatusResult {
        let ResultPayload::Raw(bytes) = payload else {
            panic!("status redaction must remain a typed raw result");
        };
        rmp_serde::from_slice(&bytes).expect("status result")
    }

    #[test]
    fn ordinary_reader_cannot_probe_unrelated_host_telemetry() {
        let redacted = decode(
            redact_resource_status_result(result("other-host"), &request("host-secret"), false)
                .unwrap(),
        );
        assert!(redacted.host_snapshot.is_none());
        assert!(redacted.host_ref.is_none());
        assert_eq!(redacted.host_revision, 0);
        assert_eq!(redacted.held_cpu_weight, 0);
        assert_eq!(redacted.held_memory_mib, 0);
        assert_eq!(redacted.held_disk_mib, 0);
        assert_eq!(redacted.held_process_slots, 0);
    }

    #[test]
    fn aggregate_reader_keeps_shared_host_totals_and_ordinary_relation_is_redacted() {
        let aggregate = decode(
            redact_resource_status_result(result("other-host"), &request("host-secret"), true)
                .unwrap(),
        );
        assert!(aggregate.host_snapshot.is_some());
        assert_eq!(aggregate.held_cpu_weight, 7);
        assert_eq!(
            aggregate
                .host_snapshot
                .as_ref()
                .expect("host snapshot")
                .held_cpu_weight,
            7
        );

        let related = decode(
            redact_resource_status_result(result("host-secret"), &request("host-secret"), false)
                .unwrap(),
        );
        assert!(related.host_snapshot.is_none());
        assert_eq!(related.held_cpu_weight, 0);
        assert_eq!(related.host_ref.as_deref(), Some("host-secret"));
    }
}

// Nested MessagePack values ride inside outer `bin` fields, which are opaque to
// the transport's top-level grammar scan. Keep a second, tighter budget here so
// every request-controlled inner decoder receives a preflight before serde sees
// an attacker-controlled collection size hint.
pub const MAX_NESTED_MSGPACK_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_NESTED_MSGPACK_ITEMS: usize = 1_000_000;
pub const MAX_SCREEN_OBSERVATION_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_SCREEN_OBSERVATION_ITEMS: usize = 140_128;
pub const MAX_SCREEN_PNG_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SCREEN_ELEMENTS: usize = 10_000;
pub const MAX_SCREEN_SESSION_ID_BYTES: usize = 256;
pub const MAX_SCREEN_PREVIOUS_ID_BYTES: usize = 512;
pub const MAX_SCREEN_ROLE_BYTES: usize = 256;
pub const MAX_SCREEN_ELEMENT_NAME_BYTES: usize = 4_096;
pub const MAX_SCREEN_TOTAL_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SCREEN_DIMENSION: u32 = 32_768;
pub const MAX_SCREEN_PIXELS: u64 = 100_000_000;
pub const MAX_SCREEN_COORDINATE_ABS: i64 = 10_000_000;

/// Keep the physical host ledger shared across tenants while making status
/// responses tenant-safe.  A controller with the explicit aggregate-read
/// capability may reconcile global capacity; an ordinary resource reader gets
/// host telemetry only when one of its returned reservations proves a relation
/// to that host, and never receives aggregate held totals.
pub fn redact_resource_status_result(
    mut result: medulla_result_types::epistemic_operations::ResourceReservationStatusResult,
    request: &medulla_result_types::epistemic_operations::ResourceReservationStatusRequest,
    aggregate_allowed: bool,
) -> Result<ResultPayload, String> {
    if !aggregate_allowed {
        let host_visible = request.host_ref.as_deref().is_some_and(|host_ref| {
            result
                .reservations
                .iter()
                .any(|reservation| reservation.host_ref == host_ref)
        });
        result.held_cpu_weight = 0;
        result.held_memory_mib = 0;
        result.held_disk_mib = 0;
        result.held_process_slots = 0;
        if !host_visible {
            result.host_snapshot = None;
            result.host_ref = None;
            result.host_revision = 0;
        } else {
            // A tenant-visible reservation proves only a relation to this
            // physical host. It does not authorize probing private inventory
            // labels/aliases or shared capacity/telemetry.  Do not construct
            // a zeroed pseudo-snapshot: heartbeat TTL and target identity have
            // schema invariants, and an invalid redacted object is worse than
            // an omitted one. Aggregate reconciliation is the explicit
            // controller capability below.
            result.host_snapshot = None;
        }
    }
    ResultPayload::of::<
        medulla_protocol_types::result_contract::coordination::ResourceReservationStatus,
    >(result)
}

// ── Fleet server registry (CONCEPT:MEDULLA-KG.sharding.server-registry, W2.5) ──────────
// `RegisterServer.name` is bounded by `medulla_protocol_types::result_contract::cluster::
// is_valid_server_name`, the one definition the registry, its cursors and the
// fleet catalog's discovery records share.
// `url` is an opaque endpoint reference (never a raw credentialed URL -- callers
// pass the same kind of privacy-safe reference au's `persistence_reference`
// produces), bounded generously for a reference string.
pub const MAX_REGISTER_SERVER_URL_BYTES: usize = 2_048;
// `resources_json` is non-sensitive opaque metadata (mirrors au's
// `_mcp_persistence_resources`), bounded well under the msgpack node-property cap.
pub const MAX_REGISTER_SERVER_RESOURCES_BYTES: usize = 16 * 1024;
// Lease bounds: at least 1 second, at most 24 hours -- a caller renews well inside
// this window (the stale-lease reaper never waits longer than the registered TTL).
pub const MIN_REGISTER_SERVER_TTL_SECS: u64 = 1;
pub const MAX_REGISTER_SERVER_TTL_SECS: u64 = 24 * 60 * 60;
