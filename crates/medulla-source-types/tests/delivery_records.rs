//! MEDULLA-SDLC-R019.2: delivery records round-trip and refuse unknown values.

use medulla_source_types::delivery::{decode_delivery, DELIVERY_RECORD_INVALID};
use serde_json::json;

fn record() -> serde_json::Value {
    json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "MEDULLA-SDLC-R019", "scope_rows": ["MEDULLA-SDLC-R019.2"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "owner",
            "policy_version": "1", "actor": "p1", "scope_rows": ["MEDULLA-SDLC-R019.2"],
            "project_mode": false},
        "proposals": ["pr-1"], "follows": [],
        "assignments": [{"schema_version": 1, "id": "a1", "run_trace_id": "r1",
            "state": "ended", "report": "ready", "started_at": 1, "ended_at": 2,
            "ended_reason": "merged"}],
        "inputs": [], "attention": ["review_submitted", "head_pushed"],
        "ended": "merged", "created_at": 0
    })
}

#[test]
fn delivery_records_round_trip_and_refuse_unknown_values() {
    let delivery = decode_delivery(record()).expect("valid record decodes");
    let again = serde_json::to_value(&delivery).unwrap();
    assert_eq!(again, record());
    assert_eq!(decode_delivery(again).unwrap(), delivery);

    for (pointer, bad) in [
        ("/mandate/strength", json!("implied")),
        ("/assignments/0/state", json!("paused")),
        ("/assignments/0/report", json!("done")),
        ("/ended", json!("abandoned")),
        ("/attention/0", json!("label_added")),
        ("/schema_version", json!(2)),
    ] {
        let mut value = record();
        *value.pointer_mut(pointer).unwrap() = bad;
        let refusal = decode_delivery(value).expect_err(pointer);
        assert_eq!(refusal.code(), DELIVERY_RECORD_INVALID, "{pointer}");
    }
}
