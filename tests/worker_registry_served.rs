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

use medulla_types::scheduler_config::SchedulerConfig;
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
        task_lease_ms: 60_000,
        retry_after_ms: 250,
        hydration_scope_cap: 10_000,
        hydration_split_target_files: 2_000,
        architecture_view_max_containers: 512,
        enrich_timeout_ms: 5_000,
        history_max_commits: 100_000,
        history_support_floor: 3,
        history_confidence_floor_milli: 300,
        history_max_files_per_commit: 200,
        process_import_max_elements: 100_000,
        process_document_max_bytes: 16 * 1024 * 1024,
        process_document_max_depth: 64,
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R075
#[test]
fn non_reserved_task_is_refused_when_only_reserved_capacity_remains() {
    let record = WorkerRecord {
        worker_id: "worker-1".into(),
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
