//! Served reserved-worker admission (MEDULLA-DURABLE-KERNEL-R075).
//!
//! `AdmitWorkerTask` answers through `medulla_work_types::worker_registry`'s
//! admission decision over `granted_slots` (MEDULLA-PLACEMENT-R002.3): a
//! one-worker pool is granted the general lane, and a non-reserved task is
//! refused with `ADMIT_HELD_FOR_RESERVE` when only reserved capacity remains —
//! the exception is a lane with queued work and no running worker. The served
//! dispatch arm and the registry/config slots in `ServerState` land with
//! MEDULLA-DURABLE-KERNEL-R074.2/R076.2.

use std::collections::BTreeMap;

use medulla_work_types::scheduler_config::SchedulerConfig;
use medulla_work_types::worker_registry::{
    admit_reserved_worker_task, LaneLoad, WorkerAdmission, WorkerAdmissionRefusal, WorkerRecord,
    WorkerSizingInput,
};

fn sched(total_workers: u32, general_floor: Option<u32>) -> SchedulerConfig {
    let mut lane_floors = BTreeMap::new();
    if let Some(floor) = general_floor {
        lane_floors.insert("general".to_string(), floor);
    }
    SchedulerConfig {
        total_workers,
        lane_floors,
        cores_per_mille: 1000,
        memory_per_worker_bytes: 1024 * 1024 * 1024,
        max_queue_depth: 64,
        analytics_workers: 1,
        analytics_lease_ms: 60_000,
        analytics_poll_ms: 250,
        task_lease_ms: 60_000,
        retry_after_ms: 250,
        worker_ttl_ms: 30_000,
        analytics_slots: 1,
        hydration_scope_cap: 10_000,
        hydration_split_target_files: 2_000,
        architecture_view_max_containers: 512,
        enrich_timeout_ms: 5_000,
        history_max_commits: 100_000,
        history_support_floor: 3,
        history_confidence_floor_milli: 300,
        history_max_files_per_commit: 200,
        process_import_max_elements: 100_000,
        process_compile_max_nodes: 100_000,
        process_document_max_bytes: 16 * 1024 * 1024,
        process_document_max_depth: 64,
        changeset_max_blob_bytes:
            medulla_work_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES,
        tree_checkpoint_interval:
            medulla_work_types::scheduler_config::DEFAULT_TREE_CHECKPOINT_INTERVAL,
        index_max_working_bytes:
            medulla_work_types::scheduler_config::DEFAULT_INDEX_MAX_WORKING_BYTES,
        ann_storage_ceiling_bytes:
            medulla_work_types::scheduler_config::DEFAULT_ANN_STORAGE_CEILING_BYTES,
        ann_hub_pruning: medulla_work_types::scheduler_config::AnnHubPruning::Disabled,
        orchestration_limits: Default::default(),
        ingest_retry: Default::default(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R075, MEDULLA-PLACEMENT-R002.600
#[test]
fn non_reserved_task_is_refused_when_only_reserved_capacity_remains() {
    let record = WorkerRecord {
        worker_id: "worker-1".into(),
        host_iri: "host:alpha".into(),
        cores: 16,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        lanes_supported: ["general".into()].into(),
        last_heartbeat_ms: 0,
        expires_at_ms: u64::MAX,
    };
    // A one-worker pool: two slots, one reserved for the general lane.
    let config = sched(2, Some(1));
    let input = WorkerSizingInput {
        request_ceiling: Some(1),
        available_memory_bytes: record.memory_bytes,
        is_write_lane: false,
    };

    // The pool's single worker is granted the general lane.
    let grant = admit_reserved_worker_task(
        &record,
        &config,
        &BTreeMap::new(),
        input,
        9,
        "grant-1".into(),
    );
    let WorkerAdmission::Granted(grant) = grant else {
        panic!("one-worker pool was refused: {grant:?}")
    };
    assert_eq!(grant.slots, 1);
    assert_eq!(grant.reserved_lane, "general");
    assert_eq!(grant.lease_seconds, 60);
    assert_eq!(grant.retry_after_seconds, 0);
    assert_eq!(grant.grant_id, "grant-1");

    // With the one non-reserved slot taken, only reserved capacity remains:
    // the non-reserved task is held for the reserve.
    let mut load = BTreeMap::new();
    load.insert(
        "general".to_string(),
        LaneLoad {
            running: 1,
            queued: 0,
        },
    );
    let held = admit_reserved_worker_task(&record, &config, &load, input, 9, "grant-2".into());
    let WorkerAdmission::Refused(WorkerAdmissionRefusal::AdmitHeldForReserve { lane }) = held
    else {
        panic!("expected the reserve refusal: {held:?}")
    };
    assert_eq!(lane, "general");
    assert_eq!(
        WorkerAdmissionRefusal::AdmitHeldForReserve { lane: lane.clone() }.code(),
        "ADMIT_HELD_FOR_RESERVE"
    );

    // The exception: a lane with queued work and no running worker is
    // relieved even when the grant would starve the reserve.
    let idle_config = sched(1, Some(1));
    let mut idle = BTreeMap::new();
    idle.insert(
        "general".to_string(),
        LaneLoad {
            running: 0,
            queued: 2,
        },
    );
    let relieved =
        admit_reserved_worker_task(&record, &idle_config, &idle, input, 9, "grant-3".into());
    assert!(
        matches!(relieved, WorkerAdmission::Granted(_)),
        "queued idle lane was held: {relieved:?}"
    );

    // A hard capacity refusal keeps its code and binding limit.
    let starved = WorkerSizingInput {
        available_memory_bytes: 0,
        ..input
    };
    let refused = admit_reserved_worker_task(
        &record,
        &config,
        &BTreeMap::new(),
        starved,
        9,
        "grant-4".into(),
    );
    let WorkerAdmission::Refused(WorkerAdmissionRefusal::WorkerCapacityZero { limit }) = refused
    else {
        panic!("expected the capacity refusal: {refused:?}")
    };
    assert_eq!(limit, "INSUFFICIENT_MEMORY");
    assert_eq!(
        WorkerAdmissionRefusal::WorkerCapacityZero {
            limit: limit.clone()
        }
        .code(),
        "WORKER_CAPACITY_ZERO"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R074.2
#[cfg(feature = "jobs")]
#[test]
fn unknown_expired_and_cross_owner_requests_refuse() {
    use std::sync::atomic::{AtomicU64, Ordering};

    use medulla_h_jobs::worker_registry::{heartbeat, register, Clock};
    use medulla_work_types::worker_registry::{OwnedWorkerRegistry, WorkerRegistration};
    use parking_lot::Mutex;

    struct FakeClock(AtomicU64);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    const TTL_MS: u64 = 30_000;
    let clock = FakeClock(AtomicU64::new(1_000));
    let registry = Mutex::new(OwnedWorkerRegistry::default());
    let registration = || WorkerRegistration {
        worker_id: "worker-1".into(),
        host_iri: "host:alpha".into(),
        cores: 8,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        lanes_supported: ["general".to_string()].into(),
    };
    let refusal = |response: medulla_protocol_types::protocol::Response| {
        response.refusal_text().expect("the call must be refused")
    };

    // The owner registers; a live worker cannot be taken over.
    let registered = register(&registry, &clock, 1, "principal-a", TTL_MS, registration());
    assert!(registered.error.is_none(), "{:?}", registered.error);
    assert!(refusal(register(
        &registry,
        &clock,
        2,
        "principal-b",
        TTL_MS,
        registration()
    ))
    .contains("WORKER_ALREADY_REGISTERED"));

    // An unknown worker and another principal's worker are refused.
    assert!(refusal(heartbeat(
        &registry,
        &clock,
        3,
        "principal-a",
        TTL_MS,
        "worker-unknown"
    ))
    .contains("WORKER_UNKNOWN"));
    assert!(refusal(heartbeat(
        &registry,
        &clock,
        4,
        "principal-b",
        TTL_MS,
        "worker-1"
    ))
    .contains("ACCESS_DENIED"));

    // The owner extends the lease while it is live.
    clock.0.store(1_000 + TTL_MS - 1, Ordering::Relaxed);
    let renewed = heartbeat(&registry, &clock, 5, "principal-a", TTL_MS, "worker-1");
    assert!(renewed.error.is_none(), "{:?}", renewed.error);

    // Past the ttl the worker disappears and its heartbeat is refused.
    let past_ttl = 1_000 + 2 * TTL_MS;
    clock.0.store(past_ttl, Ordering::Relaxed);
    assert!(registry.lock().get_live("worker-1", past_ttl).is_none());
    assert!(refusal(heartbeat(
        &registry,
        &clock,
        6,
        "principal-a",
        TTL_MS,
        "worker-1"
    ))
    .contains("WORKER_EXPIRED"));

    // An expired worker registers anew under another principal, which then owns it.
    let taken = register(&registry, &clock, 7, "principal-b", TTL_MS, registration());
    assert!(taken.error.is_none(), "{:?}", taken.error);
    assert!(refusal(heartbeat(
        &registry,
        &clock,
        8,
        "principal-a",
        TTL_MS,
        "worker-1"
    ))
    .contains("ACCESS_DENIED"));
}

#[cfg(all(
    feature = "server",
    feature = "security",
    feature = "redb",
    feature = "jobs"
))]
mod common;
#[cfg(all(
    feature = "server",
    feature = "security",
    feature = "redb",
    feature = "jobs"
))]
#[path = "common/test_support.rs"]
mod test_support;

// MEDULLA-PLACEMENT-R002.4: the served `AdmitWorkerTask` over a redb-backed state.
#[cfg(all(
    feature = "server",
    feature = "security",
    feature = "redb",
    feature = "jobs"
))]
mod admit_served {
    use super::*;

    use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_work_types::worker_registry::WorkerRegistration;

    use super::{common, test_support};

    const SECRET: &str = "medulla-placement-r002-4-admit-worker-task-secret";

    fn admission(response: &Response) -> WorkerAdmission {
        assert!(
            response.error.is_none(),
            "unexpected refusal: {:?} {:?}",
            response.error,
            response.error_detail
        );
        match &response.result {
            Some(ResultPayload::Raw(bytes)) => {
                rmp_serde::from_slice(bytes).expect("msgpack admission")
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    fn admit(worker: &str) -> Method {
        Method::AdmitWorkerTask {
            worker_id: worker.to_string(),
            requested_max: 1,
        }
    }

    // spec: MEDULLA-PLACEMENT-R002.4
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn served_admit_worker_task_grants_general_lane_and_reads_lane_load() {
        common::configure_authority();
        let dir = test_support::fresh_dir("placement-r002-4-admit");
        let (_backend, state) = test_support::redb_state_at(
            SECRET,
            common::current_isolation(),
            dir.to_str().expect("utf-8 temp dir"),
        );
        let tenant = common::test_tenant();
        let owner = VerifiedRequestContext::verified_for_test_in_tenant("agent-a", tenant);
        let other = VerifiedRequestContext::verified_for_test_in_tenant("agent-b", tenant);

        // The stored scheduler policy: two workers, one slot reserved for `general`.
        {
            let current = state.read().await;
            let store = medulla_persistence::scheduler_config_store::shared_scheduler_config_store(
                &current,
            )
            .expect("scheduler config store");
            medulla_h_jobs::scheduler_config::apply_config(&store, sched(2, Some(1)))
                .expect("store the scheduler policy");
        }

        let registered = medulla_h_jobs::worker_registry::try_handle(
            &state,
            1,
            &owner,
            Method::WorkerRegister {
                registration: WorkerRegistration {
                    worker_id: "worker-1".into(),
                    host_iri: "host:alpha".into(),
                    cores: 16,
                    memory_bytes: 8 * 1024 * 1024 * 1024,
                    lanes_supported: ["general".to_string()].into(),
                },
            },
        )
        .await
        .expect("worker registry method");
        assert!(registered.error.is_none(), "{:?}", registered.error);

        let first = medulla_h_jobs::worker_registry::try_handle(&state, 2, &owner, admit("worker-1"))
            .await
            .expect("worker registry method");
        let WorkerAdmission::Granted(grant) = admission(&first) else {
            panic!("a one-worker pool was refused")
        };
        assert_eq!(grant.slots, 1);
        assert_eq!(grant.reserved_lane, "general");
        assert!(grant.grant_id.starts_with("grant:"), "{}", grant.grant_id);

        // One task of queue `general` is enqueued and claimed: the lane has one
        // running worker and no queued work, so only reserved capacity remains.
        let enqueued = medulla_h_jobs::task_queue::try_handle(
            &state,
            3,
            &owner,
            Method::TaskQueueEnqueue {
                queue: "general".into(),
                kind: "scan".into(),
                payload_ref: "blob:1".into(),
                idempotency_key: "k1".into(),
                priority: 5,
                not_before_ms: 0,
            },
        )
        .await
        .expect("task queue method");
        assert!(enqueued.error.is_none(), "{:?}", enqueued.error);
        let claimed = medulla_h_jobs::task_queue::try_handle(
            &state,
            4,
            &owner,
            Method::TaskQueueClaim {
                worker_id: "worker-1".into(),
                grant_id: grant.grant_id.clone(),
                lease_ms: 60_000,
            },
        )
        .await
        .expect("task queue method");
        assert!(claimed.error.is_none(), "{:?}", claimed.error);

        let second = medulla_h_jobs::worker_registry::try_handle(&state, 5, &owner, admit("worker-1"))
            .await
            .expect("worker registry method");
        match admission(&second) {
            WorkerAdmission::Refused(WorkerAdmissionRefusal::AdmitHeldForReserve { lane }) => {
                assert_eq!(lane, "general")
            }
            other => panic!("expected the reserve refusal: {other:?}"),
        }

        // Another principal's call for the same worker is refused.
        let denied = medulla_h_jobs::worker_registry::try_handle(&state, 6, &other, admit("worker-1"))
            .await
            .expect("worker registry method");
        assert!(denied.result.is_none());
        assert!(denied
            .error
            .expect("cross-owner admission is refused")
            .contains("ACCESS_DENIED"));
    }
}
