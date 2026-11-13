//! Served proofs of the clustering kernel (MEDULLA-CLUSTER).

use medulla_protocol_types::protocol::{Method, ResultPayload};

use super::medulla_wire_wire_tests::wired_catalog_tests::create_test_graph;
use super::medulla_wire_wire_tests::wired_catalog_tests::request;
use super::medulla_wire_wire_tests::wired_catalog_tests::test_state;
use super::medulla_wire_wire_tests::wired_catalog_tests::CREATOR;

const GRAPH: &str = "clusters";

fn two_blobs() -> Vec<Vec<f64>> {
    let mut data = Vec::new();
    for i in 0..20 {
        let wobble = f64::from(i % 5) * 0.05;
        data.push(vec![0.0 + wobble, 0.0 - wobble]);
        data.push(vec![9.0 - wobble, 9.0 + wobble]);
    }
    data
}

async fn ds_kmeans(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    id: u64,
    data: Vec<Vec<f64>>,
    k: usize,
) -> medulla_protocol_types::protocol::Response {
    medulla_server_router::dispatch::request_boundary::dispatch(
        state,
        request(
            id,
            GRAPH,
            Method::DsKMeans {
                data,
                k,
                max_iter: 50,
            },
        ),
    )
    .await
}

fn labels_of(resp: &medulla_protocol_types::protocol::Response) -> Vec<usize> {
    assert!(resp.error.is_none(), "dispatch error: {:?}", resp.error);
    match &resp.result {
        Some(ResultPayload::Json(v)) => serde_json::from_value(v["labels"].clone()).unwrap(),
        other => panic!("expected Json result, got {other:?}"),
    }
}

// spec: MEDULLA-CLUSTER-R001.3
#[tokio::test(flavor = "multi_thread")]
async fn ds_kmeans_served_separates_blobs_and_refuses_k_above_rows() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    create_test_graph(&state, GRAPH, 1).await;

    let first = labels_of(&ds_kmeans(&state, 2, two_blobs(), 2).await);
    let again = labels_of(&ds_kmeans(&state, 3, two_blobs(), 2).await);
    assert_eq!(first, again, "a repeat call returns the same labels");
    // Even rows are blob A, odd rows blob B.
    assert!(first.iter().step_by(2).all(|&l| l == first[0]));
    assert!(first.iter().skip(1).step_by(2).all(|&l| l == first[1]));
    assert_ne!(first[0], first[1]);

    let refused = ds_kmeans(&state, 4, vec![vec![0.0], vec![1.0]], 5).await;
    assert_eq!(refused.error.as_deref(), Some("CLUSTERING_K_EXCEEDS_ROWS"));
    let nan = ds_kmeans(&state, 5, vec![vec![0.0], vec![f64::NAN], vec![2.0]], 2).await;
    assert_eq!(nan.error.as_deref(), Some("CLUSTERING_INPUT_NOT_FINITE"));
}

/// Index activation proofs (MEDULLA-CLUSTER-R002.6, R011.5).
#[cfg(feature = "ann")]
mod activation {
    use super::*;
    use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
    use medulla_decision_types::decision::DecisionPolicy;
    use medulla_semantic_index::semantic::{Progress, ProgressSink};
    use medulla_server_graph::semantic_activation::{
        activate_one, cancel_all_builds, cluster_config, sweep_resident_graphs_with,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const DIM: usize = 16;
    const ROWS: usize = 5000;

    /// A small deterministic generator of unit-interval floats.
    struct Stream(u64);
    impl Stream {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        }
        fn direction(&mut self) -> Vec<f32> {
            (0..DIM).map(|_| self.next()).collect()
        }
    }

    fn centres() -> Vec<Vec<f32>> {
        let mut stream = Stream(0x9E37_79B9_7F4A_7C15);
        (0..12).map(|_| stream.direction()).collect()
    }

    async fn graph_core(
        state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        graph: &str,
        id: u64,
    ) -> Arc<medulla_graph_core::graph::GraphCore> {
        create_test_graph(state, graph, id).await;
        state
            .read()
            .await
            .registry
            .get(graph)
            .expect("the graph is registered")
            .core
            .clone()
    }

    /// `ROWS` vectors tightly clustered around twelve centres.
    fn seed_clustered(core: &medulla_graph_core::graph::GraphCore) {
        let centres = centres();
        let mut stream = Stream(7);
        let mut store = core.semantic_store.write();
        for i in 0..ROWS {
            let v: Vec<f32> = centres[i % 12]
                .iter()
                .map(|x| x + stream.next() * 0.02)
                .collect();
            store.add_embedding(format!("n{i}"), v).unwrap();
        }
    }

    /// Activate the graph's index with the engine's resolved knobs, then ingest vectors
    /// from a wider distribution than the trained one, ten at a time, until the index goes
    /// stale or `limit` rows were added. Returns the rows added, the drift ratio and
    /// whether the index is stale.
    async fn ingest_until_stale(
        state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        graph: &str,
        id: u64,
        limit: usize,
    ) -> (usize, f64, bool) {
        let core = graph_core(state, graph, id).await;
        seed_clustered(&core);
        let cluster = cluster_config(&*state.read().await);
        let built = {
            let (name, core) = (graph.to_string(), core.clone());
            tokio::task::spawn_blocking(move || activate_one(&name, &core, None, &cluster))
                .await
                .unwrap()
        };
        assert!(built, "the activation builds the index");
        assert!(core.semantic_store.read().is_ready());
        // Vectors around the same centres with three times the spread: a shifted
        // distribution whose drift ratio crosses 1.5 within a few thousand rows.
        let centres = centres();
        let mut stream = Stream(99);
        let mut added = 0;
        while added < limit {
            for _ in 0..10 {
                let v: Vec<f32> = centres[added % 12]
                    .iter()
                    .map(|x| x + stream.next() * 0.06)
                    .collect();
                core.semantic_store
                    .write()
                    .add_embedding(format!("far{added}"), v)
                    .unwrap();
                added += 1;
            }
            if core.semantic_store.read().ann_stale() {
                break;
            }
        }
        let store = core.semantic_store.read();
        (
            added,
            store.ann_drift_ratio().unwrap_or(0.0),
            store.ann_stale(),
        )
    }

    // spec: MEDULLA-CLUSTER-R002.6
    #[tokio::test(flavor = "multi_thread")]
    async fn activation_uses_pinned_cluster_knobs() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        // No pinned policy: the knob defaults give the default drift ratio of 1.5, and
        // shifted ingest marks the index stale as soon as its ratio crosses it.
        let unpinned = test_state(&[CREATOR]);
        assert!(unpinned.read().await.pinned_policy.is_none());
        assert!((cluster_config(&*unpinned.read().await).refresh_drift_ratio - 1.5).abs() < 1e-9);
        let (added, ratio, stale) = ingest_until_stale(&unpinned, "unpinned", 10, 4000).await;
        assert!(stale, "the shifted ingest never marked the index stale");
        assert!((1.5..9.0).contains(&ratio), "crossing ratio {ratio}");

        // A pinned policy raises the drift ratio above that crossing: the same build and
        // the same ingest leave the index fresh, so the activation read the pinned knob.
        let pinned = test_state(&[CREATOR]);
        let mut policy = DecisionPolicy::engine_default();
        let threshold = ratio + 0.5;
        policy.knobs.insert(
            KnobName::new("cluster.refresh_drift_ratio"),
            KnobValue::DecimalMicros((threshold * 1_000_000.0) as i64),
        );
        pinned.write().await.pinned_policy = Some(Arc::new(policy));
        let resolved = cluster_config(&*pinned.read().await);
        assert!((resolved.refresh_drift_ratio - threshold).abs() < 1e-5);
        let (same_rows, pinned_ratio, pinned_stale) =
            ingest_until_stale(&pinned, "pinned", 11, added).await;
        assert_eq!(same_rows, added);
        assert!(
            (pinned_ratio - ratio).abs() < 1e-9,
            "same build, same ratio"
        );
        assert!(
            !pinned_stale,
            "the pinned threshold {threshold} was ignored"
        );
    }

    /// Holds the training thread inside a report until released.
    struct Hold {
        reached: AtomicBool,
        release: AtomicBool,
    }

    impl ProgressSink for Hold {
        fn report(&self, _p: &Progress) {
            self.reached.store(true, Ordering::SeqCst);
            let began = Instant::now();
            while !self.release.load(Ordering::SeqCst) && began.elapsed() < Duration::from_secs(30)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    // spec: MEDULLA-CLUSTER-R011.5
    #[tokio::test(flavor = "multi_thread")]
    async fn ready_before_index_build_finishes() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = test_state(&[CREATOR]);
        let core = graph_core(&state, "boot", 20).await;
        seed_clustered(&core);

        // The boot sweep spawns the build and returns; the build is held mid-training.
        let hold = Arc::new(Hold {
            reached: AtomicBool::new(false),
            release: AtomicBool::new(false),
        });
        let began = Instant::now();
        let spawned = sweep_resident_graphs_with(&state, Some(hold.clone())).await;
        assert_eq!(spawned, 1, "the sweep found the graph that needs an index");
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "the sweep waited {:?} for the build",
            began.elapsed()
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        while !hold.reached.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the build never began training");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Readiness did not wait: the engine answers, the graph has no index yet and
        // serves by exact search.
        assert!(core.semantic_store.read().is_warming());
        assert!(!core.semantic_store.read().is_ready());
        let probe = vec![0.25f32; DIM];
        core.semantic_store
            .write()
            .add_embedding("late".into(), probe.clone())
            .unwrap();
        let hits = core.semantic_store.read().semantic_search(&probe, 3);
        assert_eq!(hits.first().map(|h| h.0.as_str()), Some("late"));

        // Release the build; it ends and installs the index, and shutdown has nothing left.
        hold.release.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(120);
        while !core.semantic_store.read().is_ready() {
            assert!(Instant::now() < deadline, "the build never finished");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(cancel_all_builds(), 0);
    }
}
