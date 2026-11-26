//! Served task-queue methods over the durable job store
//! (MEDULLA-DURABLE-KERNEL-R072).
//!
//! The handlers are a typed facade over the one queue store, `JobStore`. The
//! restart proof drives the handlers over a store that is dropped and reopened
//! from its file: the lease is a durable row, so an unacked claim redelivers
//! after the lease lapses and after the restart. The served proof drives the
//! three methods through the real `dispatch` surface and checks the scope gate.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "redb",
    feature = "jobs"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::path::Path;

use medulla_h_jobs::task_queue::{ack, claim, enqueue, renew, TaskQueueCall};
use medulla_h_jobs::worker_admission::AdmissionInputs;
use medulla_jobs::JobStore;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_source_types::ingest_job::IngestRetryPolicy;
use medulla_work_types::scheduler_config::SchedulerConfig;
use medulla_work_types::worker_registry::{WorkerRegistration, WorkerRegistry};
use medulla_work_types::task_queue::{
    ClaimedTask, TaskAck, TaskAcked, TaskClaim, TaskEnqueue, TaskEnqueued, TaskOutcome,
    TaskRenew, TaskRenewed,
};

const SECRET: &str = "medulla-durable-kernel-r072-task-queue-secret";
const T0: i64 = 1_000_000;

fn open(dir: &Path) -> JobStore {
    let authority = medulla_kernel::store_authority::process_authority();
    JobStore::open_in_dir(
        dir,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("open the job store")
}

fn call(tenant: &str, now_ms: i64) -> TaskQueueCall<'_> {
    TaskQueueCall {
        req_id: 1,
        tenant,
        principal: "ingest-service",
        now_ms,
    }
}

/// The ingest retry policy the ack of a non-ingest queue never reads.
fn unused_ingest_policy() -> Result<IngestRetryPolicy, String> {
    Err("the queues of this test are not ingest queues".to_string())
}

/// Admission inputs with `workers` registered live and no lane load.
fn admission(workers: &[&str]) -> AdmissionInputs {
    let mut registry = WorkerRegistry::default();
    for worker in workers {
        registry
            .register(
                WorkerRegistration {
                    worker_id: (*worker).to_string(),
                    host_iri: "host:alpha".into(),
                    cores: 16,
                    memory_bytes: 8 * 1024 * 1024 * 1024,
                    lanes_supported: ["general".to_string()].into(),
                },
                0,
                u64::MAX / 2,
            )
            .unwrap();
    }
    let total_workers = 8;
    AdmissionInputs {
        registry,
        config: SchedulerConfig {
        total_workers,
        lane_floors: BTreeMap::new(),
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
        document_max_bytes: 67_108_864,
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
        admit_retry_ms: medulla_work_types::scheduler_config::DEFAULT_ADMIT_RETRY_MS,
    },
        lane_load: BTreeMap::new(),
        shard_writer_width: 4,
    }
}

fn decode<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    assert!(
        response.error.is_none(),
        "unexpected refusal: {:?} {:?}",
        response.error,
        response.error_detail
    );
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!("unexpected payload: {other:?}"),
    }
}

fn refusal_code(response: &Response) -> &str {
    assert!(response.result.is_none(), "expected a refusal");
    response.error.as_deref().expect("a refusal code")
}

fn task(key: &str) -> TaskEnqueue {
    TaskEnqueue {
        queue: "ingest.scan".into(),
        kind: "scan".into(),
        payload_ref: format!("blob:{key}"),
        idempotency_key: key.into(),
        priority: 5,
        not_before_ms: 0,
    }
}

fn lease(worker: &str, lease_ms: u64) -> TaskClaim {
    TaskClaim {
        worker_id: worker.into(),
        grant_id: "grant-1".into(),
        lease_ms,
    }
}

fn ack_of(task_id: &str, worker: &str, outcome: TaskOutcome) -> TaskAck {
    TaskAck {
        task_id: task_id.into(),
        worker_id: worker.into(),
        outcome,
        detail: String::new(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R072
#[test]
fn unacked_claim_redelivers_after_lease_expiry_and_restart() {
    common::configure_authority();
    let dir = test_support::fresh_dir("r072-task-queue-restart");
    let tenant = "tenant-a";

    let store = open(&dir);
    let enqueued: TaskEnqueued = decode(&enqueue(&store, &call(tenant, T0), task("k1")));
    let duplicate = enqueue(&store, &call(tenant, T0), task("k1"));
    assert_eq!(refusal_code(&duplicate), "TASK_DUPLICATE");

    let first: Option<ClaimedTask> = decode(&claim(&store, &call(tenant, T0), lease("w1", 1_000), &admission(&["w1", "w2", "w9"])));
    let first = first.expect("the task is leased");
    assert_eq!(first.task_id, enqueued.task_id);
    assert_eq!(first.queue, "ingest.scan");
    assert_eq!(first.kind, "scan");
    assert_eq!(first.payload_ref, "blob:k1");
    assert_eq!(first.attempt, 1);
    assert_eq!(first.lease_expires_at_ms, T0 + 1_000);

    // Another tenant's claim sees no task, even with a live task queued.
    let other: Option<ClaimedTask> =
        decode(&claim(&store, &call("tenant-b", T0), lease("w9", 1_000), &admission(&["w1", "w2", "w9"])));
    assert!(other.is_none(), "a cross-tenant claim returned a task");

    // The process restarts with the claim unacked.
    drop(store);
    let store = open(&dir);

    // Before the lease lapses nothing is ready; after it lapses the same task
    // is leased again to another worker.
    let early: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, T0 + 500), lease("w2", 1_000), &admission(&["w1", "w2", "w9"])));
    assert!(early.is_none(), "a live lease was redelivered");
    let later = T0 + 1_001;
    let second: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, later), lease("w2", 1_000), &admission(&["w1", "w2", "w9"])));
    let second = second.expect("the lapsed lease redelivers");
    assert_eq!(second.task_id, enqueued.task_id);
    assert_eq!(second.attempt, 2);

    // The first worker no longer holds the task; the second one does.
    let stale = ack(
        &store,
        &call(tenant, later),
        ack_of(&enqueued.task_id, "w1", TaskOutcome::Succeeded),
        &unused_ingest_policy(),
    );
    assert_eq!(refusal_code(&stale), "TASK_ACK_NOT_CLAIMED");
    let acked: TaskAcked = decode(&ack(
        &store,
        &call(tenant, later + 1),
        ack_of(&enqueued.task_id, "w2", TaskOutcome::Succeeded),
        &unused_ingest_policy(),
    ));
    assert_eq!(acked.task_id, enqueued.task_id);
    assert_eq!(acked.outcome, TaskOutcome::Succeeded);
    let none: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, later + 2), lease("w2", 1_000), &admission(&["w1", "w2", "w9"])));
    assert!(none.is_none(), "an acked task was delivered again");
}

fn renew_of(task_id: &str, worker: &str, lease_ms: u64) -> TaskRenew {
    TaskRenew {
        task_id: task_id.into(),
        worker_id: worker.into(),
        lease_ms,
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R072.1
#[test]
fn renew_extends_lease_and_refuses_expired_or_foreign() {
    common::configure_authority();
    let dir = test_support::fresh_dir("r072-1-task-queue-renew");
    let tenant = "tenant-a";
    let store = open(&dir);
    let enqueued: TaskEnqueued = decode(&enqueue(&store, &call(tenant, T0), task("r1")));
    let first: Option<ClaimedTask> = decode(&claim(&store, &call(tenant, T0), lease("w1", 1_000), &admission(&["w1", "w2", "w9"])));
    let first = first.expect("the task is leased");
    assert_eq!(first.attempt, 1);

    // A renew extends the expiry to now + lease_ms and keeps the attempt.
    let renewed: TaskRenewed = decode(&renew(
        &store,
        &call(tenant, T0 + 500),
        renew_of(&enqueued.task_id, "w1", 5_000),
    ));
    assert_eq!(renewed.task_id, enqueued.task_id);
    assert_eq!(renewed.lease_expires_at_ms, T0 + 500 + 5_000);

    // Past the original expiry the renewed lease still holds: nothing redelivers.
    let held: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, T0 + 2_000), lease("w2", 1_000), &admission(&["w1", "w2", "w9"])));
    assert!(held.is_none(), "a renewed lease was redelivered");

    // Another worker, another tenant, an unknown task and a zero lease refuse.
    let foreign = renew(
        &store,
        &call(tenant, T0 + 2_000),
        renew_of(&enqueued.task_id, "w2", 1_000),
    );
    assert_eq!(refusal_code(&foreign), "TASK_ACK_NOT_CLAIMED");
    let other_tenant = renew(
        &store,
        &call("tenant-b", T0 + 2_000),
        renew_of(&enqueued.task_id, "w1", 1_000),
    );
    assert_eq!(refusal_code(&other_tenant), "TASK_ACK_NOT_CLAIMED");
    let unknown = renew(
        &store,
        &call(tenant, T0 + 2_000),
        renew_of("no-such-task", "w1", 1_000),
    );
    assert_eq!(refusal_code(&unknown), "TASK_ACK_NOT_CLAIMED");
    let zero = renew(
        &store,
        &call(tenant, T0 + 2_000),
        renew_of(&enqueued.task_id, "w1", 0),
    );
    assert_eq!(refusal_code(&zero), "TASK_LEASE_ZERO");

    // After the renewed lease lapses, the same worker's renew refuses expired.
    let lapsed = renew(
        &store,
        &call(tenant, T0 + 500 + 5_001),
        renew_of(&enqueued.task_id, "w1", 1_000),
    );
    assert_eq!(refusal_code(&lapsed), "TASK_ACK_LEASE_EXPIRED");

    // The task redelivers with a higher attempt: renew never counted as one.
    let again: Option<ClaimedTask> = decode(&claim(
        &store,
        &call(tenant, T0 + 500 + 5_001),
        lease("w2", 1_000),
        &admission(&["w1", "w2", "w9"]),
    ));
    assert_eq!(again.expect("redelivered").attempt, 2);
}

// spec: MEDULLA-DURABLE-KERNEL-R072
#[test]
fn ack_outcomes_map_to_release_fail_and_lapse() {
    common::configure_authority();
    let dir = test_support::fresh_dir("r072-task-queue-outcomes");
    let store = open(&dir);
    let tenant = "tenant-a";

    // Retry releases the task and keeps its attempt count.
    let retried: TaskEnqueued = decode(&enqueue(&store, &call(tenant, T0), task("retry")));
    let leased: Option<ClaimedTask> = decode(&claim(&store, &call(tenant, T0), lease("w1", 5_000), &admission(&["w1", "w2", "w9"])));
    assert_eq!(leased.expect("leased").attempt, 1);
    let _: TaskAcked = decode(&ack(
        &store,
        &call(tenant, T0 + 1),
        ack_of(&retried.task_id, "w1", TaskOutcome::Retry),
        &unused_ingest_policy(),
    ));
    let again: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, T0 + 2), lease("w1", 5_000), &admission(&["w1", "w2", "w9"])));
    let again = again.expect("a released task is leased again");
    assert_eq!(again.task_id, retried.task_id);
    assert_eq!(again.attempt, 2);

    // Failed ends the task for good.
    let _: TaskAcked = decode(&ack(
        &store,
        &call(tenant, T0 + 3),
        ack_of(&retried.task_id, "w1", TaskOutcome::Failed),
        &unused_ingest_policy(),
    ));
    let gone: Option<ClaimedTask> =
        decode(&claim(&store, &call(tenant, T0 + 4), lease("w1", 5_000), &admission(&["w1", "w2", "w9"])));
    assert!(gone.is_none(), "a failed task was delivered again");

    // An ack after the lease lapsed is refused, and so is an ack of a task the
    // worker never held.
    let lapsed: TaskEnqueued = decode(&enqueue(&store, &call(tenant, T0), task("lapse")));
    let _: Option<ClaimedTask> = decode(&claim(&store, &call(tenant, T0 + 10), lease("w1", 1_000), &admission(&["w1", "w2", "w9"])));
    let late = ack(
        &store,
        &call(tenant, T0 + 10 + 1_000),
        ack_of(&lapsed.task_id, "w1", TaskOutcome::Succeeded),
        &unused_ingest_policy(),
    );
    assert_eq!(refusal_code(&late), "TASK_ACK_LEASE_EXPIRED");
    let unknown = ack(
        &store,
        &call(tenant, T0 + 11),
        ack_of("job-ffffffffffffffff", "w1", TaskOutcome::Succeeded),
        &unused_ingest_policy(),
    );
    assert_eq!(refusal_code(&unknown), "TASK_ACK_NOT_CLAIMED");

    // The bounds refuse before the queue is touched.
    let mut blank = task("blank");
    blank.queue.clear();
    assert_eq!(
        refusal_code(&enqueue(&store, &call(tenant, T0), blank)),
        "TASK_QUEUE_NAME_BLANK"
    );
    assert_eq!(
        refusal_code(&claim(&store, &call(tenant, T0), lease("w1", 0), &admission(&["w1", "w2", "w9"]))),
        "TASK_LEASE_ZERO"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R072
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn served_methods_enqueue_claim_and_ack_behind_their_scopes() {
    common::configure_authority();
    let dir = test_support::fresh_dir("r072-task-queue-served");
    let (_backend, state) = test_support::redb_state_at(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );
    let graph = "r072-served";

    let enqueue_method = Method::TaskQueueEnqueue {
        queue: "ingest.scan".into(),
        kind: "scan".into(),
        payload_ref: "blob:served".into(),
        idempotency_key: "served-1".into(),
        priority: 5,
        not_before_ms: 0,
    };
    let enqueued: TaskEnqueued = decode(
        &test_support::dispatch(
            &state,
            test_support::request(SECRET, 1, graph, enqueue_method),
        )
        .await,
    );
    let claimed: Option<ClaimedTask> = decode(
        &test_support::dispatch(
            &state,
            test_support::request(
                SECRET,
                2,
                graph,
                Method::TaskQueueClaim {
                    worker_id: "w1".into(),
                    grant_id: "grant-1".into(),
                    lease_ms: 60_000,
                },
            ),
        )
        .await,
    );
    assert_eq!(
        claimed.expect("the served claim leases the task").task_id,
        enqueued.task_id
    );
    let acked: TaskAcked = decode(
        &test_support::dispatch(
            &state,
            test_support::request(
                SECRET,
                3,
                graph,
                Method::TaskQueueAck {
                    task_id: enqueued.task_id.clone(),
                    worker_id: "w1".into(),
                    outcome: TaskOutcome::Succeeded,
                    detail: String::new(),
                    drain: None,
                },
            ),
        )
        .await,
    );
    assert_eq!(acked.outcome, TaskOutcome::Succeeded);

    // A caller holding neither task scope is refused before the handler runs.
    let refused = test_support::dispatch(
        &state,
        common::signed_request_with_scopes(
            SECRET,
            4,
            graph,
            &["graph:read"],
            Method::TaskQueueClaim {
                worker_id: "w1".into(),
                grant_id: "grant-1".into(),
                lease_ms: 60_000,
            },
        ),
    )
    .await;
    assert!(refused.result.is_none(), "an unscoped caller claimed");
    let error = refused.error.expect("the unscoped caller is refused");
    assert!(
        error.contains("ACCESS_DENIED") || error.contains("task:work"),
        "unexpected refusal: {error}"
    );
}
