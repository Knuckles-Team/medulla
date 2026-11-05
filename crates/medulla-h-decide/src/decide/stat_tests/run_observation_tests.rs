//! MEDULLA-ORCH-VOCAB-R009.18.2 — observation-window ingest through the direct
//! `AddNode` admission.

use medulla_graph_core::graph::GraphCore;
use medulla_h_graph::harness_admission::direct_add_node;
use medulla_protocol_types::protocol::Method;

fn unknown() -> serde_json::Value {
    serde_json::json!({"state": "unknown", "value": "not reported"})
}

fn measured(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"state": "measured", "value": value})
}

fn window(run_revision: u64, eligible: u64) -> serde_json::Value {
    serde_json::json!({
        "node_type": "RunObservationWindow",
        "run_id": "trace-1",
        "run_revision": run_revision,
        "window_id": "window-1",
        "window_revision": 1,
        "start_ms": 0,
        "end_ms": 1000,
        "latency_samples_ms": measured(serde_json::json!([5, 7])),
        "error_count": measured(serde_json::json!(0)),
        "eligible_attempts": measured(serde_json::json!(eligible)),
        "usage": {
            "input_tokens": unknown(),
            "output_tokens": unknown(),
            "cache_read_tokens": unknown(),
            "cache_write_tokens": unknown(),
        },
        "cost": unknown(),
        "model_ref": unknown(),
        "profile_revision": unknown(),
        "width": unknown(),
        "budget_consumed": unknown(),
        "budget_remaining": unknown(),
        "capacity_receipt_ref": unknown(),
    })
}

fn add(node_id: &str, props: &serde_json::Value) -> Method {
    Method::AddNode {
        node_id: node_id.into(),
        properties_msgpack: rmp_serde::to_vec_named(props).unwrap(),
    }
}

fn commit(core: &GraphCore, method: &Method) {
    let Method::AddNode {
        node_id,
        properties_msgpack,
    } = method
    else {
        panic!("AddNode came back")
    };
    core.add_node(node_id.clone(), properties_msgpack.clone());
}

// spec: MEDULLA-ORCH-VOCAB-R009.18.2
#[test]
fn window_ingest_and_refusals() {
    let core = GraphCore::new();
    let trace = serde_json::json!({"node_type": "RunTrace", "run_revision": 2});
    commit(
        &core,
        &direct_add_node(&add("trace-1", &trace), &core, 1).unwrap(),
    );

    let admitted = direct_add_node(&add("window-1", &window(2, 0)), &core, 5).unwrap();
    let Method::AddNode {
        properties_msgpack, ..
    } = &admitted
    else {
        panic!("AddNode came back")
    };
    let stored: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
    assert_eq!(stored["run_trace_ref"], "trace-1");
    assert_eq!(stored["error_rate"]["state"], "unknown");
    commit(&core, &admitted);

    let replay = direct_add_node(&add("window-1", &window(2, 0)), &core, 9).unwrap();
    let Method::AddNode {
        properties_msgpack: replayed,
        ..
    } = &replay
    else {
        panic!("AddNode came back")
    };
    assert_eq!(
        replayed, properties_msgpack,
        "an equal re-send is idempotent"
    );

    let mismatch = direct_add_node(
        &add("window-2", &{
            let mut w = window(3, 1);
            w["window_id"] = "window-2".into();
            w
        }),
        &core,
        9,
    )
    .unwrap_err();
    assert!(
        mismatch.starts_with("WINDOW_RUN_REVISION_MISMATCH"),
        "{mismatch}"
    );
    assert!(!core.has_node("window-2"), "a refusal writes nothing");

    let conflict = direct_add_node(&add("window-1", &window(2, 4)), &core, 9).unwrap_err();
    assert!(
        conflict.starts_with("WINDOW_CONTENT_CONFLICT"),
        "{conflict}"
    );
}
