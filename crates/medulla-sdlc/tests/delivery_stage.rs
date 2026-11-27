//! MEDULLA-SDLC-R019.7: report recording and the derived delivery stage.

use std::collections::BTreeSet;

use medulla_sdlc::lifecycle::delivery::stage::{derive_stage, record_answer, record_report};
use medulla_source_types::delivery::{
    decode_delivery, AssignmentReport, AssignmentState, Delivery, DeliveryInput,
    DeliveryOutcome, DeliveryProposal, DeliveryQuestion, DeliveryReportRefusal,
    DeliveryReportState, DeliveryReportWrite, DeliveryStage, FeedbackKind, InputSource,
    ProposalState,
};
use serde_json::json;

fn base() -> Delivery {
    decode_delivery(json!({
        "schema_version": 1, "id": "d1", "tenant": "t1", "repo": "medulla",
        "slice_root": "MEDULLA-SDLC-R019", "scope_rows": ["MEDULLA-SDLC-R019.7"],
        "mandate": {"schema_version": 1, "strength": "explicit", "basis": "owner",
            "policy_version": "1", "actor": "p1", "scope_rows": ["MEDULLA-SDLC-R019.7"],
            "project_mode": "development"},
        "seq": 0, "proposals": [], "follows": [],
        "assignments": [{"schema_version": 1, "id": "a1", "run_trace_id": "r1",
            "state": "running", "report": null, "started_at": 1, "ended_at": null,
            "ended_reason": null}],
        "inputs": [], "attention": null, "ended": null, "created_at": 0
    }))
    .expect("fixture decodes")
}

fn report(state: DeliveryReportState, seq: u64) -> AssignmentReport {
    AssignmentReport {
        state,
        summary: "s".into(),
        proposal: None,
        evidence: vec![],
        seq,
    }
}

fn with_report(state: DeliveryReportState, seq: u64) -> Delivery {
    let mut d = base();
    d.seq = seq;
    d.assignments[0].report = Some(report(state, seq));
    d
}

fn proposal(state: ProposalState) -> DeliveryProposal {
    DeliveryProposal {
        id: "pr-1".into(),
        state,
    }
}

fn person_input(seq: u64) -> DeliveryInput {
    DeliveryInput {
        event_id: "e1".into(),
        kind: FeedbackKind::ReviewComment,
        source: InputSource::Person,
        seq,
        detail: None,
    }
}

// spec: MEDULLA-SDLC-R019.7
#[test]
fn stage_table_first_match_wins() {
    // ended
    let mut d = base();
    d.ended = Some(DeliveryOutcome::Merged);
    assert_eq!(derive_stage(&d), DeliveryStage::Ended);
    // ended beats an open question
    d.attention = Some(DeliveryQuestion {
        text: "q".into(),
        raised_by: None,
        seq: 1,
    });
    assert_eq!(derive_stage(&d), DeliveryStage::Ended);
    // person/answer, and it beats a ready report with an open proposal
    d.ended = None;
    d.proposals = vec![proposal(ProposalState::Open)];
    d.assignments[0].report = Some(report(DeliveryReportState::Ready, 2));
    assert_eq!(derive_stage(&d), DeliveryStage::PersonAnswer);
    // person/review, and it beats blocked/queued rules
    d.attention = None;
    assert_eq!(derive_stage(&d), DeliveryStage::PersonReview);
    d.assignments[0].state = AssignmentState::Pending;
    assert_eq!(derive_stage(&d), DeliveryStage::PersonReview);
    // ready with no open proposal falls through to the later rules
    d.proposals = vec![proposal(ProposalState::Merged)];
    assert_eq!(derive_stage(&d), DeliveryStage::WorkingQueued);
    // person/blocked and person/failed
    let mut blocked = with_report(DeliveryReportState::Blocked, 2);
    assert_eq!(derive_stage(&blocked), DeliveryStage::PersonBlocked);
    blocked.assignments[0].state = AssignmentState::Pending;
    assert_eq!(derive_stage(&blocked), DeliveryStage::PersonBlocked);
    let failed = with_report(DeliveryReportState::Failed, 2);
    assert_eq!(derive_stage(&failed), DeliveryStage::PersonFailed);
    // a later person input lifts blocked: running assignment is working, pending is queued
    let mut lifted = with_report(DeliveryReportState::Blocked, 2);
    lifted.inputs = vec![person_input(3)];
    assert_eq!(derive_stage(&lifted), DeliveryStage::Working);
    lifted.assignments[0].state = AssignmentState::Pending;
    assert_eq!(derive_stage(&lifted), DeliveryStage::WorkingQueued);
    // an automation input does not lift it
    let mut automation = with_report(DeliveryReportState::Blocked, 2);
    automation.inputs = vec![DeliveryInput {
        source: InputSource::Automation,
        ..person_input(3)
    }];
    assert_eq!(derive_stage(&automation), DeliveryStage::PersonBlocked);
    // working/queued: no assignment running
    let mut queued = base();
    queued.assignments[0].state = AssignmentState::Pending;
    assert_eq!(derive_stage(&queued), DeliveryStage::WorkingQueued);
    // working/queued beats idle
    let mut idle_queued = with_report(DeliveryReportState::Waiting, 2);
    idle_queued.assignments[0].state = AssignmentState::Pending;
    assert_eq!(derive_stage(&idle_queued), DeliveryStage::WorkingQueued);
    // idle
    assert_eq!(
        derive_stage(&with_report(DeliveryReportState::Waiting, 2)),
        DeliveryStage::Idle
    );
    // working
    assert_eq!(derive_stage(&base()), DeliveryStage::Working);
    assert_eq!(
        derive_stage(&with_report(DeliveryReportState::Progress, 2)),
        DeliveryStage::Working
    );
    // the latest report is the one with the highest seq over all assignments
    let mut two = with_report(DeliveryReportState::Blocked, 2);
    let mut second = two.assignments[0].clone();
    second.id = "a2".into();
    second.report = Some(report(DeliveryReportState::Progress, 5));
    two.assignments.push(second);
    assert_eq!(derive_stage(&two), DeliveryStage::Working);
    // wire names
    let names: Vec<&str> = [
        DeliveryStage::Ended,
        DeliveryStage::PersonAnswer,
        DeliveryStage::PersonReview,
        DeliveryStage::PersonBlocked,
        DeliveryStage::PersonFailed,
        DeliveryStage::WorkingQueued,
        DeliveryStage::Idle,
        DeliveryStage::Working,
    ]
    .iter()
    .map(|s| s.as_str())
    .collect();
    assert_eq!(
        names,
        [
            "ended",
            "person/answer",
            "person/review",
            "person/blocked",
            "person/failed",
            "working/queued",
            "idle",
            "working"
        ]
    );
    assert_eq!(
        serde_json::to_value(DeliveryStage::PersonReview).unwrap(),
        json!("person/review")
    );
}

fn write(state: DeliveryReportState) -> DeliveryReportWrite {
    DeliveryReportWrite {
        delivery_id: "d1".into(),
        assignment_id: "a1".into(),
        state,
        summary: "s".into(),
        proposal: None,
        question: None,
        evidence: vec![],
    }
}

// spec: MEDULLA-SDLC-R019.7
#[test]
fn report_refusals_and_sequence_order() {
    let known: BTreeSet<String> = ["v1".to_owned()].into();
    let d = base();

    let mut unknown = write(DeliveryReportState::Progress);
    unknown.assignment_id = "nope".into();
    let refusal = record_report(&d, &unknown, &known).unwrap_err();
    assert!(matches!(refusal, DeliveryReportRefusal::AssignmentUnknown(_)));
    assert_eq!(refusal.code(), "ASSIGNMENT_UNKNOWN");

    let mut ended = base();
    ended.assignments[0].state = AssignmentState::Ended;
    let refusal = record_report(&ended, &write(DeliveryReportState::Progress), &known).unwrap_err();
    assert_eq!(refusal.code(), "ASSIGNMENT_ENDED");

    let refusal = record_report(&d, &write(DeliveryReportState::Ready), &known).unwrap_err();
    assert_eq!(refusal.code(), "REPORT_EVIDENCE_MISSING");
    let mut bad_evidence = write(DeliveryReportState::Ready);
    bad_evidence.evidence = vec!["v1".into(), "v9".into()];
    let refusal = record_report(&d, &bad_evidence, &known).unwrap_err();
    assert_eq!(refusal.code(), "REPORT_EVIDENCE_MISSING");
    assert_eq!(d, base(), "a refusal changes nothing");

    // a first report moves pending to running and takes seq 1
    let mut pending = base();
    pending.assignments[0].state = AssignmentState::Pending;
    let first = record_report(&pending, &write(DeliveryReportState::Progress), &known).unwrap();
    assert_eq!(first.assignments[0].state, AssignmentState::Running);
    assert_eq!(first.assignments[0].report.as_ref().unwrap().seq, 1);
    assert_eq!(first.seq, 1);

    // a new proposal ID is appended open; a second report replaces the first with the next seq
    let mut ready = write(DeliveryReportState::Ready);
    ready.proposal = Some("pr-7".into());
    ready.evidence = vec!["v1".into()];
    let second = record_report(&first, &ready, &known).unwrap();
    assert_eq!(second.assignments[0].report.as_ref().unwrap().seq, 2);
    assert_eq!(
        second.assignments[0].report.as_ref().unwrap().state,
        DeliveryReportState::Ready
    );
    assert_eq!(
        second.proposals,
        vec![DeliveryProposal {
            id: "pr-7".into(),
            state: ProposalState::Open
        }]
    );
    // the same proposal ID is not appended twice
    let third = record_report(&second, &ready, &known).unwrap();
    assert_eq!(third.proposals.len(), 1);
    assert_eq!(third.seq, 3);
    // the stage follows the report
    assert_eq!(derive_stage(&third), DeliveryStage::PersonReview);
}

// spec: MEDULLA-SDLC-R019.8
#[test]
fn one_question_replaced_or_cleared() {
    let known: BTreeSet<String> = ["v1".to_owned()].into();
    let mut d = base();
    d.inputs = vec![person_input(1)];
    d.seq = 1;
    let ask = |text: &str| {
        let mut w = write(DeliveryReportState::Waiting);
        w.question = Some(text.to_owned());
        w
    };

    // a question is raised and names the person's event
    let d = record_report(&d, &ask("which branch?"), &known).unwrap();
    let q = d.attention.clone().unwrap();
    assert_eq!(q.text, "which branch?");
    assert_eq!(q.raised_by.as_deref(), Some("e1"));
    assert_eq!(derive_stage(&d), DeliveryStage::PersonAnswer);

    // the same text reuses the open question
    let same = record_report(&d, &ask("which branch?"), &known).unwrap();
    assert_eq!(same.attention, Some(q.clone()));

    // a new text replaces it with a new seq
    let replaced = record_report(&same, &ask("which repo?"), &known).unwrap();
    let r = replaced.attention.clone().unwrap();
    assert_eq!(r.text, "which repo?");
    assert!(r.seq > q.seq);

    // a report without a question leaves it in place
    let kept = record_report(&replaced, &write(DeliveryReportState::Progress), &known).unwrap();
    assert_eq!(kept.attention, replaced.attention);

    // an explicit empty question clears it
    let cleared = record_report(&kept, &ask(""), &known).unwrap();
    assert_eq!(cleared.attention, None);

    // a ready report clears it
    let mut ready = write(DeliveryReportState::Ready);
    ready.evidence = vec!["v1".into()];
    ready.question = Some("still there?".into());
    let after_ready = record_report(&replaced, &ready, &known).unwrap();
    assert_eq!(after_ready.attention, None);

    // 600 characters pass; 601 refuse and change nothing
    let max = "x".repeat(600);
    assert!(record_report(&d, &ask(&max), &known).is_ok());
    let too_long = "é".repeat(601);
    let refusal = record_report(&d, &ask(&too_long), &known).unwrap_err();
    assert_eq!(refusal.code(), "QUESTION_TOO_LONG");
}

// spec: MEDULLA-SDLC-R019.17
#[test]
fn a_person_answer_clears_the_open_question_and_is_recorded_as_a_person_input() {
    let mut waiting = base();
    waiting.attention = Some(DeliveryQuestion {
        text: "which branch?".into(),
        raised_by: None,
        seq: 1,
    });
    waiting.seq = 1;
    assert_eq!(derive_stage(&waiting), DeliveryStage::PersonAnswer);
    let answered = record_answer(&waiting, "use main").unwrap();
    assert_eq!(answered.attention, None);
    assert_ne!(derive_stage(&answered), DeliveryStage::PersonAnswer);
    let input = answered.inputs.last().expect("the answer is an input");
    assert_eq!(input.source, InputSource::Person);
    assert_eq!(input.kind, FeedbackKind::ConversationComment);
    assert_eq!(input.detail.as_deref(), Some("use main"));
    assert_eq!(input.seq, 2);
    assert_eq!(input.event_id, "answer:d1:2");
    // The record the caller read is untouched.
    assert!(waiting.attention.is_some() && waiting.inputs.is_empty());
}

// spec: MEDULLA-SDLC-R019.17
#[test]
fn an_answer_without_a_question_or_out_of_bounds_is_refused() {
    let quiet = base();
    assert!(record_answer(&quiet, "x").unwrap_err().starts_with("DELIVERY_NO_QUESTION"));
    let mut waiting = base();
    waiting.attention = Some(DeliveryQuestion {
        text: "q".into(),
        raised_by: None,
        seq: 1,
    });
    assert!(record_answer(&waiting, "  ").unwrap_err().starts_with("ANSWER_INVALID"));
    let long = "a".repeat(2001);
    assert!(record_answer(&waiting, &long).unwrap_err().starts_with("ANSWER_INVALID"));
    waiting.ended = Some(DeliveryOutcome::Stopped);
    assert!(record_answer(&waiting, "x").unwrap_err().starts_with("DELIVERY_NO_QUESTION"));
}
