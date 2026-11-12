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
