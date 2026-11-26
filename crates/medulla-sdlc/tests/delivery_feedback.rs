//! MEDULLA-SDLC-R019.5: feedback routes to the deliveries that follow the proposal.

use std::collections::BTreeSet;

use medulla_sdlc::lifecycle::delivery::feedback::{route_feedback, FeedbackContext};
use medulla_source_types::delivery::{
    decode_delivery, Delivery, DeliveryOutcome, FeedbackKind, InputSource, ScmFeedbackEvent,
};
use serde_json::json;

fn delivery(id: &str, ended: Option<&str>) -> Delivery {
    decode_delivery(json!({
        "schema_version": 1, "id": id, "tenant": "t1", "repo": "medulla",
        "slice_root": "MEDULLA-SDLC-R019", "scope_rows": ["MEDULLA-SDLC-R019.5"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "owner",
            "policy_version": "1", "actor": "p1", "scope_rows": ["MEDULLA-SDLC-R019.5"],
            "project_mode": "development"},
        "seq": 0, "proposals": [{"id": "pr-1", "state": "open"}], "follows": [],
        "assignments": [], "inputs": [], "attention": null, "ended": ended,
        "created_at": 0
    }))
    .expect("fixture decodes")
}

fn event(id: &str, source: InputSource, failed: bool) -> ScmFeedbackEvent {
    ScmFeedbackEvent {
        event_id: id.into(),
        connector: "scm".into(),
        kind: FeedbackKind::CheckChanged,
        source,
        proposal_id: "pr-1".into(),
        failed,
        summary: "s".into(),
        observed_at: 1,
    }
}

fn context<'a>(
    connectors: &'a BTreeSet<String>,
    revalidated: &'a BTreeSet<String>,
) -> FeedbackContext<'a> {
    FeedbackContext {
        registered_connectors: connectors,
        proposal_known: true,
        already_stored: false,
        revalidated,
    }
}

// spec: MEDULLA-SDLC-R019.5
#[test]
fn events_route_to_following_deliveries_and_bots_never_resume() {
    let connectors = BTreeSet::from(["scm".to_string()]);
    let revalidated = BTreeSet::from(["ended-ok".to_string()]);
    let ctx = context(&connectors, &revalidated);
    let deliveries = vec![
        delivery("live", None),
        delivery("ended-ok", Some("completed")),
        delivery("ended-no", Some("completed")),
        delivery("stopped", Some("stopped")),
    ];
    let bot = route_feedback(&event("e1", InputSource::Automation, false), &deliveries, &ctx)
        .expect("routes");
    assert_eq!(
        bot.deliveries.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        ["live"]
    );
    assert!(bot.resumed.is_empty() && !bot.file_defect);
    assert_eq!(bot.deliveries[0].inputs[0].seq, 1);
    let person = route_feedback(&event("e2", InputSource::Person, false), &deliveries, &ctx)
        .expect("routes");
    let ids: Vec<&str> = person.deliveries.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["live", "ended-ok"]);
    assert_eq!(person.resumed, ["ended-ok"]);
    assert_eq!(person.deliveries[1].ended, None::<DeliveryOutcome>);
}

// spec: MEDULLA-SDLC-R019.5
#[test]
fn unrouted_failed_check_files_a_defect_and_refusals_are_typed() {
    let connectors = BTreeSet::from(["scm".to_string()]);
    let none = BTreeSet::new();
    let ctx = context(&connectors, &none);
    let ended = vec![delivery("ended", Some("merged"))];
    let failed = route_feedback(&event("e3", InputSource::Automation, true), &ended, &ctx)
        .expect("routes");
    assert!(failed.file_defect && failed.deliveries.is_empty());
    let passed = route_feedback(&event("e4", InputSource::Automation, false), &ended, &ctx)
        .expect("routes");
    assert!(!passed.file_defect);
    let mut stored = context(&connectors, &none);
    stored.already_stored = true;
    let again = route_feedback(&event("e3", InputSource::Automation, true), &ended, &stored)
        .expect("routes");
    assert!(again.duplicate && !again.file_defect);
    let mut unknown_connector = event("e5", InputSource::Person, false);
    unknown_connector.connector = "other".into();
    assert_eq!(
        route_feedback(&unknown_connector, &ended, &ctx).unwrap_err().code(),
        "FEEDBACK_SOURCE_UNKNOWN"
    );
    let mut no_proposal = context(&connectors, &none);
    no_proposal.proposal_known = false;
    assert_eq!(
        route_feedback(&event("e6", InputSource::Person, false), &ended, &no_proposal)
            .unwrap_err()
            .code(),
        "FEEDBACK_PROPOSAL_UNKNOWN"
    );
    let mut blank = event("", InputSource::Person, false);
    blank.event_id = " ".into();
    assert_eq!(
        route_feedback(&blank, &ended, &ctx).unwrap_err().code(),
        "FEEDBACK_EVENT_INVALID"
    );
}
