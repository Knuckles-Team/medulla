//! MEDULLA-SDLC-R019.9: automated review rounds have a budget.

use medulla_sdlc::lifecycle::delivery::rounds::{record_round_verdict, review_round_verdict};
use medulla_source_types::delivery::{decode_delivery, ReviewRecord, ReviewerKind, RoundVerdict};
use serde_json::json;

fn review(proposal: &str, reviewer: &str, kind: ReviewerKind, run: &str) -> ReviewRecord {
    ReviewRecord {
        proposal_id: proposal.into(),
        reviewer: reviewer.into(),
        reviewer_kind: kind,
        run_key: run.into(),
    }
}

// spec: MEDULLA-SDLC-R019.9
#[test]
fn fourth_automated_round_is_not_addressed() {
    let bot = |run: &str| review("pr-1", "bot", ReviewerKind::Automated, run);
    let stored = vec![bot("r1"), bot("r2"), bot("r3"), bot("r1")];
    // Three distinct runs are within the budget; a repeat of a counted run adds nothing.
    assert_eq!(
        review_round_verdict(&stored, &bot("r3"), 3),
        RoundVerdict::Address
    );
    assert_eq!(
        review_round_verdict(&stored, &bot("r1"), 3),
        RoundVerdict::Address
    );
    // The fourth distinct run is not addressed.
    assert_eq!(
        review_round_verdict(&stored, &bot("r4"), 3),
        RoundVerdict::StopAddressing
    );
    // Another proposal or reviewer counts apart; a person has no limit.
    let other_proposal = review("pr-2", "bot", ReviewerKind::Automated, "r4");
    assert_eq!(
        review_round_verdict(&stored, &other_proposal, 3),
        RoundVerdict::Address
    );
    let other_bot = review("pr-1", "bot-2", ReviewerKind::Automated, "r4");
    assert_eq!(
        review_round_verdict(&stored, &other_bot, 3),
        RoundVerdict::Address
    );
    let person = review("pr-1", "bot", ReviewerKind::Person, "r9");
    assert_eq!(
        review_round_verdict(&stored, &person, 1),
        RoundVerdict::Address
    );
}

// spec: MEDULLA-SDLC-R019.9
#[test]
fn the_verdict_becomes_an_input_of_the_delivery() {
    let delivery = decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "S", "scope_rows": ["R"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "b",
            "policy_version": "p1", "actor": "a", "scope_rows": ["R"],
            "project_mode": "development"},
        "seq": 4, "proposals": [], "follows": [], "assignments": [], "inputs": [],
        "attention": null, "ended": null, "created_at": 0
    }))
    .expect("fixture decodes");
    let next = record_round_verdict(&delivery, "ev-1", RoundVerdict::StopAddressing);
    assert_eq!(next.inputs[0].seq, 5);
    assert_eq!(
        next.inputs[0].detail.as_deref(),
        Some("round_verdict:stop_addressing")
    );
}
