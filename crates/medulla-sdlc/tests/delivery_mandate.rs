//! MEDULLA-SDLC-R019.6: a resume needs open rows, a permitted mode and the principal.

use medulla_sdlc::lifecycle::delivery::mandate::{note_policy_change, revalidate_mandate};
use medulla_source_types::delivery::{
    decode_delivery, Delivery, DeliveryOutcome, FeedbackKind, MandateFacts,
};
use medulla_source_types::lifecycle::ProjectMode;
use serde_json::json;

fn delivery(strength: &str, ended: Option<&str>) -> Delivery {
    decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "MEDULLA-SDLC-R019", "scope_rows": ["MEDULLA-SDLC-R019.6"],
        "mandate": {"schema_version": 1, "strength": strength, "basis": "b",
            "policy_version": "p1", "actor": "person-1",
            "scope_rows": ["MEDULLA-SDLC-R019.6"], "project_mode": "development"},
        "seq": 0, "proposals": [], "follows": [], "assignments": [], "inputs": [],
        "attention": null, "ended": ended, "created_at": 0
    }))
    .expect("fixture decodes")
}

fn facts(mode: ProjectMode) -> MandateFacts {
    MandateFacts {
        mode,
        rows_open: true,
        actor_can_write: true,
        policy_version: "p1".into(),
    }
}

// spec: MEDULLA-SDLC-R019.6
#[test]
fn resume_needs_open_rows_mode_and_principal() {
    let explicit = delivery("explicit", Some("completed"));
    assert!(revalidate_mandate(&explicit, &facts(ProjectMode::Development)).is_ok());
    let refused = |delivery: &Delivery, now: &MandateFacts| {
        revalidate_mandate(delivery, now).unwrap_err().code()
    };
    assert_eq!(
        refused(&explicit, &facts(ProjectMode::Reference)),
        "MANDATE_REVOKED"
    );
    let mut closed = facts(ProjectMode::Development);
    closed.rows_open = false;
    assert_eq!(refused(&explicit, &closed), "MANDATE_REVOKED");
    let mut lost = facts(ProjectMode::Development);
    lost.actor_can_write = false;
    assert_eq!(refused(&explicit, &lost), "MANDATE_REVOKED");
    let inferred = delivery("inferred", Some("completed"));
    assert_eq!(
        refused(&inferred, &facts(ProjectMode::Development)),
        "MANDATE_REVOKED"
    );
    assert!(revalidate_mandate(&inferred, &facts(ProjectMode::Evolving)).is_ok());
}

// spec: MEDULLA-SDLC-R019.6
#[test]
fn policy_change_adds_an_input_and_never_cancels() {
    let live = delivery("explicit", None);
    let mut now = facts(ProjectMode::Development);
    assert_eq!(note_policy_change(&live, &now), live);
    now.policy_version = "p2".into();
    let changed = note_policy_change(&live, &now);
    assert_eq!(changed.ended, None::<DeliveryOutcome>);
    assert_eq!(changed.inputs.len(), 1);
    assert_eq!(changed.inputs[0].kind, FeedbackKind::PolicyChanged);
    assert_eq!(
        changed.inputs[0].detail.as_deref(),
        Some("policy_version p1 -> p2")
    );
    let ended = delivery("explicit", Some("merged"));
    assert_eq!(note_policy_change(&ended, &now), ended);
}
