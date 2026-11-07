//! The lane work item derives terminality from its WorkItem machine state
//! (MEDULLA-ORCH-VOCAB-R813).

use medulla_work_types::work_item_machine::WorkItemStatus;

use super::types::{LaneDecision, LaneWorkItem};
use super::work_item::lane_work_item_status;

fn item(status: WorkItemStatus) -> LaneWorkItem {
    LaneWorkItem {
        status,
        lease_expires_at_ms: 0,
        host_ref: String::new(),
        resource_reservation_id: String::new(),
        cleanup_hold_id: None,
        cleanup_lane_id: None,
        cleanup_expected_hold_revision: None,
        lane_intent: None,
    }
}

fn decoded(stored: &str) -> Result<WorkItemStatus, LaneDecision> {
    let row: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({ "status": stored })).unwrap();
    lane_work_item_status(&row)
}

fn derived(stored: &str, terminal: bool) {
    let status = decoded(stored).expect("a WorkItem state");
    assert_eq!(status.as_stored(), stored);
    assert_eq!(item(status).terminal(), terminal, "{stored}");
    assert_eq!(item(status).terminal(), status.is_terminal());
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_is_derived_from_status() {
    for (stored, terminal) in [
        ("submitted", false),
        ("ready", false),
        ("leased", false),
        ("running", false),
        ("succeeded", true),
        ("failed", true),
        ("cancelled", true),
        ("dead_letter", true),
    ] {
        derived(stored, terminal);
    }
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_submitted() {
    derived("submitted", false);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_ready() {
    derived("ready", false);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_leased() {
    derived("leased", false);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_running() {
    derived("running", false);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_succeeded() {
    derived("succeeded", true);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_failed() {
    derived("failed", true);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_cancelled() {
    derived("cancelled", true);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn terminal_derived_dead_letter() {
    derived("dead_letter", true);
}

// spec: MEDULLA-ORCH-VOCAB-R813
#[test]
fn older_record_is_refused() {
    assert_eq!(decoded("completed"), Err(LaneDecision::Invalid));
    assert_eq!(decoded(""), Err(LaneDecision::Invalid));
}
