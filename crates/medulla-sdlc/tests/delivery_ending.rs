//! MEDULLA-SDLC-R019.11: a delivery ends with the outcome of its proposals.

use medulla_sdlc::lifecycle::delivery::ending::{apply_proposal_event, end_if_done, EndTrigger};
use medulla_source_types::delivery::{
    decode_delivery, AssignmentState, Delivery, DeliveryOutcome, ProposalEvent,
};
use serde_json::json;

fn delivery(assignment_state: &str, report: serde_json::Value, proposals: serde_json::Value) -> Delivery {
    decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "S", "scope_rows": ["R"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "b",
            "policy_version": "p1", "actor": "a", "scope_rows": ["R"],
            "project_mode": "development"},
        "seq": 1, "proposals": proposals, "follows": ["pr-1"],
        "assignments": [{"schema_version": 1, "id": "a1", "run_trace_id": "r1",
            "state": assignment_state, "report": report, "started_at": 1,
            "ended_at": null, "ended_reason": null}],
        "inputs": [], "attention": null, "ended": null, "created_at": 0
    }))
    .expect("fixture decodes")
}

fn report(state: &str, proposal: Option<&str>) -> serde_json::Value {
    json!({"state": state, "summary": "s", "proposal": proposal, "evidence": [], "seq": 1})
}

// spec: MEDULLA-SDLC-R019.11
#[test]
fn merge_of_last_open_proposal_ends_as_merged() {
    let open = delivery(
        "pending",
        json!(null),
        json!([{"id": "pr-1", "state": "open"}, {"id": "pr-2", "state": "open"}]),
    );
    let first = apply_proposal_event(&open, "pr-1", ProposalEvent::Merged).expect("applies");
    // One proposal is still open: the delivery does not end.
    let still = end_if_done(&first, EndTrigger::ProposalClosed(ProposalEvent::Merged), 9);
    assert_eq!(still.ended, None::<DeliveryOutcome>);
    let last = apply_proposal_event(&first, "pr-2", ProposalEvent::Merged).expect("applies");
    let merged = end_if_done(&last, EndTrigger::ProposalClosed(ProposalEvent::Merged), 9);
    assert_eq!(merged.ended, Some(DeliveryOutcome::Merged));
    assert_eq!(merged.assignments[0].state, AssignmentState::Ended);
    assert_eq!(merged.assignments[0].ended_at, Some(9));
    assert_eq!(merged.follows, ["pr-1"], "an ended delivery keeps its follows");
    // The last close decides: a merge then a close ends as closed.
    let mixed = apply_proposal_event(&first, "pr-2", ProposalEvent::Closed).expect("applies");
    let closed = end_if_done(&mixed, EndTrigger::ProposalClosed(ProposalEvent::Closed), 9);
    assert_eq!(closed.ended, Some(DeliveryOutcome::Closed));
    // A merged proposal is terminal: a reopen is refused.
    assert!(apply_proposal_event(&last, "pr-1", ProposalEvent::Reopened).is_err());
}

// spec: MEDULLA-SDLC-R019.11
#[test]
fn a_close_waits_for_the_running_assignment_and_reports_and_stops_end() {
    let running = delivery(
        "running",
        json!(null),
        json!([{"id": "pr-1", "state": "closed"}]),
    );
    let waiting = end_if_done(&running, EndTrigger::ProposalClosed(ProposalEvent::Closed), 5);
    assert_eq!(waiting.ended, None::<DeliveryOutcome>);
    let ready = delivery("running", report("ready", None), json!([]));
    assert_eq!(
        end_if_done(&ready, EndTrigger::Report, 5).ended,
        Some(DeliveryOutcome::Completed)
    );
    let with_proposal = delivery(
        "running",
        report("ready", Some("pr-1")),
        json!([{"id": "pr-1", "state": "open"}]),
    );
    assert_eq!(end_if_done(&with_proposal, EndTrigger::Report, 5).ended, None::<DeliveryOutcome>);
    let dropped = delivery("running", report("dropped", None), json!([]));
    assert_eq!(
        end_if_done(&dropped, EndTrigger::Report, 5).ended,
        Some(DeliveryOutcome::Dropped)
    );
    assert_eq!(
        end_if_done(&running, EndTrigger::Stopped, 5).ended,
        Some(DeliveryOutcome::Stopped)
    );
}
