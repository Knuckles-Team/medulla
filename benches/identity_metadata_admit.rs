//! Typed principal metadata admission latency (MEDULLA-IDENTITY-R023.12).
//!
//! Nightly bench group `identity/metadata_admit`: p95 below 300 microseconds per
//! record. Each iteration admits one record of each variant through
//! `identity_metadata::admit`, the step `compile_methods` runs before it compiles
//! a write set. The run fails (non-zero exit) when the p95 budget is exceeded.
//!
//! Run: cargo bench --bench identity_metadata_admit

use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::Method;
use medulla_server_api::mutation_batch::identity_metadata::{admit, MetadataCaller};

const BUDGET_P95: Duration = Duration::from_micros(300);
const SAMPLES: usize = 5_000;

fn add_node(id: &str, props: serde_json::Value) -> Method {
    Method::AddNode {
        node_id: id.to_owned(),
        properties_msgpack: rmp_serde::to_vec_named(&props).expect("properties encode"),
    }
}

fn records() -> Vec<Method> {
    vec![
        add_node(
            "pref",
            serde_json::json!({"type": "Preference", "key": "theme", "value": "dark", "share": "tenant"}),
        ),
        add_node(
            "client",
            serde_json::json!({"type": "ClientMetadata", "client_id": "cli:1", "kind": "cli",
                "version": "1.0.0", "capabilities": ["read", "write"]}),
        ),
        add_node(
            "net",
            serde_json::json!({"type": "InteractionNetwork", "subject": "usr:alice",
                "peer": "usr:alice", "relation": "collaborates", "weight_milli": 500}),
        ),
    ]
}

fn main() {
    let caller = MetadataCaller {
        principal: Some("usr:alice"),
        tenant: "acme",
    };
    let records = records();
    let mut samples = Vec::with_capacity(SAMPLES * records.len());
    for _ in 0..SAMPLES {
        for record in &records {
            let method = record.clone();
            let started = Instant::now();
            let admitted = admit(method, &caller).expect("the bench records are valid");
            samples.push(started.elapsed());
            std::hint::black_box(admitted);
        }
    }
    samples.sort();
    let p95 = samples[samples.len() * 95 / 100];
    println!("identity/metadata_admit p95 = {p95:?} (budget {BUDGET_P95:?})");
    assert!(p95 < BUDGET_P95, "admit p95 {p95:?} exceeds {BUDGET_P95:?}");
}
