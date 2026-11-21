//! Bench group `lifecycle/delivery_stage` (MEDULLA-SDLC-R019.7): `record_report` and
//! `derive_stage` timings on a delivery with many assignments.

use std::collections::BTreeSet;
use std::time::Instant;

use medulla_sdlc::lifecycle::delivery::stage::{derive_stage, record_report};
use medulla_source_types::delivery::{
    decode_delivery, DeliveryReportState, DeliveryReportWrite,
};

fn main() {
    let assignments: Vec<_> = (0..64)
        .map(|n| {
            serde_json::json!({"schema_version": 1, "id": format!("a{n}"), "run_trace_id": "r",
                "state": "running", "report": null, "started_at": 1, "ended_at": null,
                "ended_reason": null})
        })
        .collect();
    let delivery = decode_delivery(serde_json::json!({
        "schema_version": 1, "id": "d", "tenant": "t", "repo": "medulla", "slice_root": "s",
        "scope_rows": [], "mandate": {"schema_version": 1, "strength": "explicit", "basis": "b",
            "policy_version": "1", "actor": "p", "scope_rows": [], "project_mode": "development"},
        "seq": 0, "proposals": [], "follows": [], "assignments": assignments, "inputs": [],
        "attention": null, "ended": null, "created_at": 0
    }))
    .expect("bench fixture decodes");
    let write = DeliveryReportWrite {
        delivery_id: "d".into(),
        assignment_id: "a63".into(),
        state: DeliveryReportState::Progress,
        summary: "s".into(),
        proposal: Some("pr".into()),
        question: None,
        evidence: vec![],
    };
    let known = BTreeSet::new();
    let rounds = 10_000;
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(record_report(&delivery, &write, &known).expect("report applies"));
    }
    println!("lifecycle/delivery_stage record_report {:?}/op", start.elapsed() / rounds);
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(derive_stage(&delivery));
    }
    println!("lifecycle/delivery_stage derive_stage {:?}/op", start.elapsed() / rounds);
}
