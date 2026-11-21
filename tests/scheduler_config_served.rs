//! Served scheduler configuration reads (MEDULLA-DURABLE-KERNEL-R077).
//!
//! `GetShardWriterWidth` is driven through the real served `dispatch` surface
//! over a redb backend opened with a configured multi-shard layout: the reply
//! carries exactly the configured writer count `k`, and a caller whose verified
//! context lacks `scheduler:read` is refused before the handler runs.
#![cfg(all(feature = "server", feature = "security", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_work_types::worker_registry::ShardWriterWidth;

const SECRET: &str = "medulla-durable-kernel-r077-shard-writer-width-secret";
const CONFIGURED_WRITERS: usize = 4;
const GRAPH: &str = "r077-width";

fn width_method() -> Method {
    Method::GetShardWriterWidth {
        tenant: common::test_tenant().to_string(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R077
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn width_for_a_multi_shard_tenant_matches_configured_writers() {
    // The shard count is read once when the backend opens; configure k first.
    std::env::set_var("MEDULLA_REDB_SHARDS", CONFIGURED_WRITERS.to_string());
    common::configure_authority();
    let dir = test_support::fresh_dir("r077-shard-writer-width");
    let (_backend, state) = test_support::redb_state_at(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );

    let created = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            1,
            GRAPH,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
        ),
    )
    .await;
    assert!(created.error.is_none(), "CreateGraph: {:?}", created.error);

    let response = test_support::dispatch(
        &state,
        test_support::request(SECRET, 2, GRAPH, width_method()),
    )
    .await;
    assert!(
        response.error.is_none(),
        "GetShardWriterWidth: {:?} {:?}",
        response.error,
        response.error_detail
    );
    let width: ShardWriterWidth = match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack width"),
        other => panic!("unexpected width payload: {other:?}"),
    };
    assert_eq!(width.tenant, common::test_tenant());
    assert_eq!(width.shard_writers as usize, CONFIGURED_WRITERS);

    // A caller without scheduler:read is refused before the handler runs.
    let refused = test_support::dispatch(
        &state,
        common::signed_request_with_scopes(SECRET, 3, GRAPH, &["graph:read"], width_method()),
    )
    .await;
    assert!(refused.result.is_none(), "unscoped caller got a width");
    let error = refused.error.expect("unscoped caller is refused");
    assert!(
        error.contains("ACCESS_DENIED") || error.contains("scheduler:read"),
        "unexpected refusal: {error}"
    );

    // A body tenant other than the verified carrier tenant is refused.
    let foreign = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            4,
            GRAPH,
            Method::GetShardWriterWidth {
                tenant: "another-tenant".to_string(),
            },
        ),
    )
    .await;
    assert!(foreign.result.is_none(), "foreign tenant got a width");
    assert!(foreign
        .error
        .expect("foreign tenant is refused")
        .contains("ACCESS_DENIED"));
    std::env::remove_var("MEDULLA_REDB_SHARDS");
}

/// MEDULLA-DURABLE-KERNEL-R076.2: `GetSchedulerConfig` and `SetSchedulerConfig`
/// over the job store. The handlers run over a store opened from a directory, so
/// a dropped and reopened store proves the setting is durable.
#[cfg(feature = "jobs")]
mod scheduler_settings {
    use std::collections::BTreeMap;
    use std::path::Path;

    use medulla_h_jobs::scheduler_config::{effective_config, get, set};
    use medulla_jobs::JobStore;
    use medulla_protocol_types::protocol::{Response, ResultPayload};
    use medulla_work_types::scheduler_config::SchedulerConfig;
    use medulla_work_types::worker_registry::{
        admit_reserved_worker_task, LaneLoad, WorkerAdmission, WorkerAdmissionRefusal,
        WorkerRecord, WorkerSizingInput,
    };

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

    fn decode(response: &Response) -> SchedulerConfig {
        assert!(
            response.error.is_none(),
            "unexpected refusal: {:?}",
            response.error
        );
        match &response.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack"),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    fn floors(total_workers: u32, general: u32) -> SchedulerConfig {
        let base = decode(&get(
            &open(&super::test_support::fresh_dir("r076-2-baseline")),
            1,
        ));
        SchedulerConfig {
            total_workers,
            lane_floors: BTreeMap::from([("general".to_string(), general)]),
            ..base
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.2
    #[test]
    fn set_get_refuse_and_admission_follows_floors() {
        super::common::configure_authority();
        let dir = super::test_support::fresh_dir("r076-2-set-get");
        let store = open(&dir);
        let before = decode(&get(&store, 1));

        let accepted = floors(2, 1);
        assert_eq!(decode(&set(&store, 2, accepted.clone())), accepted);
        let current = decode(&get(&store, 3));
        assert_eq!(current, accepted);

        // Admission over the stored policy follows the floors.
        let record = WorkerRecord {
            worker_id: "worker-1".into(),
            cores: 16,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            lanes_supported: ["general".into()].into(),
            last_heartbeat_ms: 0,
            expires_at_ms: u64::MAX,
        };
        let input = WorkerSizingInput {
            request_ceiling: Some(1),
            available_memory_bytes: record.memory_bytes,
            is_write_lane: false,
        };
        let granted = admit_reserved_worker_task(
            &record,
            &effective_config(&store).expect("policy"),
            &BTreeMap::new(),
            input,
            9,
            "grant-1".into(),
        );
        assert!(
            matches!(granted, WorkerAdmission::Granted(_)),
            "{granted:?}"
        );
        let load = BTreeMap::from([(
            "general".to_string(),
            LaneLoad {
                running: 1,
                queued: 0,
            },
        )]);
        let held = admit_reserved_worker_task(
            &record,
            &effective_config(&store).expect("policy"),
            &load,
            input,
            9,
            "grant-2".into(),
        );
        assert!(
            matches!(
                held,
                WorkerAdmission::Refused(WorkerAdmissionRefusal::AdmitHeldForReserve { .. })
            ),
            "{held:?}"
        );

        // Floors above the pool, and a lease outside its range, refuse and
        // change nothing.
        let over = set(&store, 4, floors(2, 3));
        assert_eq!(over.error.as_deref(), Some("SCHEDULER_CONFIG_INVALID"));
        let short_lease = SchedulerConfig {
            task_lease_ms: 1,
            ..accepted.clone()
        };
        let refused = set(&store, 5, short_lease);
        assert_eq!(refused.error.as_deref(), Some("SCHEDULER_CONFIG_INVALID"));
        assert_eq!(decode(&get(&store, 6)), accepted);
        assert_ne!(before, accepted);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.2
    #[test]
    fn config_survives_restart() {
        super::common::configure_authority();
        let dir = super::test_support::fresh_dir("r076-2-restart");
        let store = open(&dir);
        let stored = SchedulerConfig {
            task_lease_ms: 90_000,
            ..floors(2, 1)
        };
        assert_eq!(decode(&set(&store, 1, stored.clone())), stored);
        drop(store);
        let reopened = open(&dir);
        assert_eq!(decode(&get(&reopened, 2)), stored);
    }
}
