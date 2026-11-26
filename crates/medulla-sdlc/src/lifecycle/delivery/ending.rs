//! A delivery ends with the outcome of its proposals (MEDULLA-SDLC-R019.11).
//!
//! A delivery ends when none of the proposals it opened stays open: `merged` when the last
//! close merged, else `closed`. A `ready` report without a proposal ends it `completed`, a
//! `dropped` report `dropped`, and a stop by a person `stopped`. A close does not end it
//! while an assignment still runs. An ended delivery keeps its `follows`, so a person's event
//! can resume it (R019.6). The functions are pure; the served arm writes the returned
//! delivery, assignments included, in one batch.

use medulla_source_types::delivery::{
    AssignmentState, Delivery, DeliveryOutcome, DeliveryReportState, ProposalEvent,
    ProposalState,
};
use medulla_types::transition_refused::TransitionRefused;

/// Why a delivery is checked for its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndTrigger {
    /// A proposal of the delivery merged or closed.
    ProposalClosed(ProposalEvent),
    /// An assignment reported.
    Report,
    /// A person stopped the delivery.
    Stopped,
}

/// Fire `event` on the proposal `proposal_id` of the delivery through the proposal machine.
/// A proposal the delivery does not hold is returned unchanged.
pub fn apply_proposal_event(
    delivery: &Delivery,
    proposal_id: &str,
    event: ProposalEvent,
) -> Result<Delivery, TransitionRefused> {
    let mut next = delivery.clone();
    if let Some(proposal) = next.proposals.iter_mut().find(|p| p.id == proposal_id) {
        proposal.state = proposal.state.apply(event)?;
    }
    Ok(next)
}

fn finish(delivery: &Delivery, outcome: DeliveryOutcome, now_ms: i64) -> Delivery {
    let mut next = delivery.clone();
    next.ended = Some(outcome);
    for assignment in &mut next.assignments {
        if assignment.state.can_move_to(AssignmentState::Ended) {
            assignment.state = AssignmentState::Ended;
            assignment.ended_at = Some(now_ms);
            assignment.ended_reason = Some("delivery ended".to_owned());
        }
    }
    next
}

/// The delivery after `trigger`: ended with its outcome, or unchanged when it is not done.
pub fn end_if_done(delivery: &Delivery, trigger: EndTrigger, now_ms: i64) -> Delivery {
    if delivery.ended.is_some() {
        return delivery.clone();
    }
    match trigger {
        EndTrigger::Stopped => finish(delivery, DeliveryOutcome::Stopped, now_ms),
        EndTrigger::ProposalClosed(event) => {
            let running = delivery
                .assignments
                .iter()
                .any(|a| a.state == AssignmentState::Running);
            let any_open = delivery
                .proposals
                .iter()
                .any(|p| p.state == ProposalState::Open);
            if running || any_open || delivery.proposals.is_empty() {
                return delivery.clone();
            }
            let outcome = if event == ProposalEvent::Merged {
                DeliveryOutcome::Merged
            } else {
                DeliveryOutcome::Closed
            };
            finish(delivery, outcome, now_ms)
        }
        EndTrigger::Report => {
            let latest = delivery
                .assignments
                .iter()
                .filter_map(|a| a.report.as_ref())
                .max_by_key(|report| report.seq);
            match latest {
                Some(report) if report.state == DeliveryReportState::Dropped => {
                    finish(delivery, DeliveryOutcome::Dropped, now_ms)
                }
                Some(report)
                    if report.state == DeliveryReportState::Ready
                        && report.proposal.is_none()
                        && delivery.proposals.is_empty() =>
                {
                    finish(delivery, DeliveryOutcome::Completed, now_ms)
                }
                _ => delivery.clone(),
            }
        }
    }
}
