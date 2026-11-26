//! MEDULLA-SDLC-R019.13: restart recovery never repeats an outside effect blindly.

use std::collections::BTreeSet;

use medulla_sdlc::lifecycle::delivery::recovery::{
    expired_by_lane_leases, lane_work_item_id, recover_assignments, LaneLease, RecordedEffect,
    HOST_RESTARTED,
};
use medulla_source_types::delivery::{
    decode_delivery, AssignmentState, ReceiptState,
};
use serde_json::json;

#[test]
// spec: MEDULLA-SDLC-R019.13
fn expired_assignment_restarts_with_note_and_unread_inputs_once() {
    let delivery = decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "S", "scope_rows": ["R"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "b",
            "policy_version": "p1", "actor": "a", "scope_rows": ["R"],
            "project_mode": "development"},
        "seq": 3, "proposals": [], "follows": [],
        "assignments": [
            {"schema_version": 1, "id": "a1", "run_trace_id": "r1", "state": "running",
             "report": {"state": "progress", "summary": "s", "proposal": null,
                        "evidence": [], "seq": 1},
             "started_at": 1, "ended_at": null, "ended_reason": null},
            {"schema_version": 1, "id": "a2", "run_trace_id": "r2", "state": "running",
             "report": null, "started_at": 1, "ended_at": null, "ended_reason": null}
        ],
        "inputs": [
            {"event_id": "e1", "kind": "review_comment", "source": "person", "seq": 1},
            {"event_id": "e2", "kind": "review_comment", "source": "person", "seq": 2},
            {"event_id": "e3", "kind": "check_changed", "source": "automation", "seq": 3}
        ],
        "attention": null, "ended": null, "created_at": 0
    }))
    .expect("fixture decodes");
    let expired = BTreeSet::from(["a1".to_string()]);
    let effects = vec![
        RecordedEffect {
            assignment_id: "a1".into(),
            description: "opened pr".into(),
            receipt: Some("receipt-1".into()),
        },
        RecordedEffect {
            assignment_id: "a1".into(),
            description: "posted comment".into(),
            receipt: None,
        },
        RecordedEffect {
            assignment_id: "a2".into(),
            description: "other".into(),
            receipt: None,
        },
    ];
    let mint = |old: &str| format!("run-for-{old}");
    let recovered = recover_assignments(&delivery, &expired, &effects, &mint, 77);
    assert_eq!(recovered.assignments.len(), 3);
    let old = &recovered.assignments[0];
    assert_eq!(old.state, AssignmentState::Ended);
    assert_eq!(old.ended_reason.as_deref(), Some(HOST_RESTARTED));
    assert_eq!(old.ended_at, Some(77));
    assert_eq!(recovered.assignments[1].state, AssignmentState::Running, "lease held");
    let fresh = &recovered.assignments[2];
    assert_eq!(fresh.state, AssignmentState::Pending);
    assert_eq!(fresh.run_trace_id, "run-for-a1");
    let note = fresh.recovery.as_ref().expect("a recovery note");
    assert_eq!(note.replaces, "a1");
    assert_eq!(note.moved_inputs, [2, 3], "the first input was read at report seq 1");
    let receipts: Vec<ReceiptState> = note.effects.iter().map(|e| e.receipt).collect();
    assert_eq!(receipts, [ReceiptState::Confirmed, ReceiptState::Uncertain]);
    // A second recovery pass moves nothing again.
    let again = recover_assignments(&recovered, &expired, &effects, &mint, 99);
    assert_eq!(again, recovered);
}

#[test]
// spec: MEDULLA-SDLC-R019.13
fn a_lapsed_lane_lease_names_its_running_assignment_by_delivery_id() {
    let delivery = decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "S", "scope_rows": ["R"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "b",
            "policy_version": "p1", "actor": "a", "scope_rows": ["R"],
            "project_mode": "development"},
        "seq": 0, "proposals": [], "follows": [],
        "assignments": [
            {"schema_version": 1, "id": "a1", "run_trace_id": "r1", "state": "running",
             "report": null, "started_at": 1, "ended_at": null, "ended_reason": null},
            {"schema_version": 1, "id": "a2", "run_trace_id": "r2", "state": "running",
             "report": null, "started_at": 1, "ended_at": null, "ended_reason": null},
            {"schema_version": 1, "id": "a3", "run_trace_id": "r3", "state": "running",
             "report": null, "started_at": 1, "ended_at": null, "ended_reason": null},
            {"schema_version": 1, "id": "a4", "run_trace_id": "r4", "state": "ended",
             "report": null, "started_at": 1, "ended_at": 2, "ended_reason": "done"}
        ],
        "inputs": [], "attention": null, "ended": null, "created_at": 0
    }))
    .expect("fixture decodes");
    let lease = |index: usize, expires_at_ms: i64, active: bool| LaneLease {
        work_item_id: lane_work_item_id(&delivery, &delivery.assignments[index]),
        expires_at_ms,
        active,
    };
    assert_eq!(lane_work_item_id(&delivery, &delivery.assignments[0]), "d1/r1");
    let leases = vec![
        lease(0, 50, true),   // passed its expiry
        lease(1, 500, true),  // still live
        lease(3, 10, false),  // lapsed, but its assignment is already ended
        LaneLease { work_item_id: "other/r9".into(), expires_at_ms: 1, active: false },
    ];
    let expired = expired_by_lane_leases(&[delivery.clone()], &leases, 100);
    assert_eq!(expired, BTreeSet::from(["a1".to_string()]), "a3 holds no lane lease");
    // A hold that is no longer active lapses whatever its expiry says.
    let released = vec![lease(1, 500, false)];
    assert_eq!(
        expired_by_lane_leases(&[delivery], &released, 100),
        BTreeSet::from(["a2".to_string()])
    );
}
