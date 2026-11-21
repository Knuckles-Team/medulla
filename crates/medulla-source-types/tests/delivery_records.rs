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
        "seq": 3, "proposals": [{"id": "pr-1", "state": "merged"}], "follows": [],
        "assignments": [{"schema_version": 1, "id": "a1", "run_trace_id": "r1",
            "state": "ended", "report": {"state": "ready", "summary": "done", "proposal": "pr-1", "evidence": ["v1"], "seq": 2}, "started_at": 1, "ended_at": 2,
            "ended_reason": "merged"}],
        "inputs": [{"event_id": "e1", "kind": "head_pushed", "source": "person", "seq": 1}],
        "attention": {"text": "which branch?", "raised_by": "e1", "seq": 3},
        "ended": "merged", "created_at": 0
    })
}

// spec: MEDULLA-SDLC-R019.2
#[test]
fn delivery_records_round_trip_and_refuse_unknown_values() {
    let delivery = decode_delivery(record()).expect("valid record decodes");
    let again = serde_json::to_value(&delivery).unwrap();
    assert_eq!(again, record());
    assert_eq!(decode_delivery(again).unwrap(), delivery);

    for (pointer, bad) in [
        ("/mandate/strength", json!("implied")),
        ("/assignments/0/state", json!("paused")),
        ("/assignments/0/report/state", json!("done")),
        ("/ended", json!("abandoned")),
        ("/inputs/0/kind", json!("label_added")),
        ("/inputs/0/source", json!("bot")),
        ("/proposals/0/state", json!("abandoned")),
        ("/schema_version", json!(2)),
    ] {
        let mut value = record();
        *value.pointer_mut(pointer).unwrap() = bad;
        let refusal = decode_delivery(value).expect_err(pointer);
        assert_eq!(refusal.code(), DELIVERY_RECORD_INVALID, "{pointer}");
    }
}
