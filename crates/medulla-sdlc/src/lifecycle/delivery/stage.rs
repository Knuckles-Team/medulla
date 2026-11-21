//! Assignment reports and the derived delivery stage (MEDULLA-SDLC-R019.7).
//!
//! [`record_report`] applies one agent report to a delivery and returns the new
//! record; it writes nothing. [`derive_stage`] is a pure read that computes the stage
//! on each call. The stage is never stored.

use std::collections::BTreeSet;

use medulla_source_types::delivery::{
    AssignmentReport, AssignmentState, Delivery, DeliveryProposal, DeliveryReportRefusal,
    DeliveryReportState, DeliveryReportWrite, DeliveryStage, InputSource, ProposalState,
};

/// Apply one report to `delivery`. `known_verdicts` is the set of gate verdict IDs of the
/// delivery's proposals.
pub fn record_report(
    delivery: &Delivery,
    write: &DeliveryReportWrite,
    known_verdicts: &BTreeSet<String>,
) -> Result<Delivery, DeliveryReportRefusal> {
    let index = delivery
        .assignments
        .iter()
        .position(|a| a.id == write.assignment_id)
        .ok_or_else(|| DeliveryReportRefusal::AssignmentUnknown(write.assignment_id.clone()))?;
    if delivery.assignments[index].state == AssignmentState::Ended {
        return Err(DeliveryReportRefusal::AssignmentEnded(
            write.assignment_id.clone(),
        ));
    }
    if write.state == DeliveryReportState::Ready {
        if write.evidence.is_empty() {
            return Err(DeliveryReportRefusal::EvidenceMissing(
                "a ready report needs evidence".to_owned(),
            ));
        }
        if let Some(unknown) = write.evidence.iter().find(|id| !known_verdicts.contains(*id)) {
            return Err(DeliveryReportRefusal::EvidenceMissing(format!(
                "unknown verdict {unknown}"
            )));
        }
    }

    let mut next = delivery.clone();
    let seq = next.next_seq();
    let assignment = &mut next.assignments[index];
    if assignment.state.can_move_to(AssignmentState::Running) {
        assignment.state = AssignmentState::Running;
    }
    assignment.report = Some(AssignmentReport {
        state: write.state,
        summary: write.summary.clone(),
        proposal: write.proposal.clone(),
        evidence: write.evidence.clone(),
        seq,
    });
    if let Some(id) = &write.proposal {
        if !next.proposals.iter().any(|p| &p.id == id) {
            next.proposals.push(DeliveryProposal {
                id: id.clone(),
                state: ProposalState::Open,
            });
        }
    }
    Ok(next)
}

/// The report with the highest `seq` over every assignment of the delivery.
fn latest_report(delivery: &Delivery) -> Option<&AssignmentReport> {
    delivery
        .assignments
        .iter()
        .filter_map(|a| a.report.as_ref())
        .max_by_key(|r| r.seq)
}

/// Derive the stage of a delivery. The first matching rule wins.
pub fn derive_stage(delivery: &Delivery) -> DeliveryStage {
    if delivery.ended.is_some() {
        return DeliveryStage::Ended;
    }
    if delivery.attention.is_some() {
        return DeliveryStage::PersonAnswer;
    }
    let latest = latest_report(delivery);
    if let Some(report) = latest {
        if report.state == DeliveryReportState::Ready
            && delivery
                .proposals
                .iter()
                .any(|p| p.state == ProposalState::Open)
        {
            return DeliveryStage::PersonReview;
        }
        if matches!(
            report.state,
            DeliveryReportState::Blocked | DeliveryReportState::Failed
        ) && !delivery
            .inputs
            .iter()
            .any(|i| i.source == InputSource::Person && i.seq > report.seq)
        {
            return if report.state == DeliveryReportState::Blocked {
                DeliveryStage::PersonBlocked
            } else {
                DeliveryStage::PersonFailed
            };
        }
    }
    if !delivery
        .assignments
        .iter()
        .any(|a| a.state == AssignmentState::Running)
    {
        return DeliveryStage::WorkingQueued;
    }
    if latest.is_some_and(|r| r.state == DeliveryReportState::Waiting) {
        return DeliveryStage::Idle;
    }
    DeliveryStage::Working
}
