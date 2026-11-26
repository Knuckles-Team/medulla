//! Restart recovery never repeats an outside effect blindly (MEDULLA-SDLC-R019.13).
//!
//! A running assignment whose lane lease expired ends with reason `host_restarted`. The engine
//! opens a new pending assignment that carries a recovery note: the outside effects the old
//! assignment recorded with their receipt state (an effect without a receipt is `uncertain`
//! and must be reconciled before a retry), and the inputs the old assignment had not read,
//! which move to the new assignment once.

use std::collections::BTreeSet;

use medulla_source_types::delivery::{
    AssignmentState, Delivery, DeliveryAssignment, ReceiptState, RecoveredEffect, RecoveryNote,
    DELIVERY_SCHEMA_VERSION,
};
use medulla_source_types::lifecycle::sha256_hex;

/// The reason a restart gives for ending an assignment.
pub const HOST_RESTARTED: &str = "host_restarted";

/// One outside effect an assignment recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEffect {
    pub assignment_id: String,
    pub description: String,
    /// The receipt of the effect, when the outside system gave one.
    pub receipt: Option<String>,
}

/// Recover the assignments of `delivery` whose lease is in `expired`. `mint_run_trace` maps
/// the id of an ended assignment to the run trace id arbora minted for its replacement.
/// A second call finds the replacement already there and changes nothing.
pub fn recover_assignments(
    delivery: &Delivery,
    expired: &BTreeSet<String>,
    effects: &[RecordedEffect],
    mint_run_trace: &dyn Fn(&str) -> String,
    now_ms: i64,
) -> Delivery {
    let mut next = delivery.clone();
    let replaced: BTreeSet<String> = delivery
        .assignments
        .iter()
        .filter_map(|a| a.recovery.as_ref().map(|note| note.replaces.clone()))
        .collect();
    let mut opened: Vec<DeliveryAssignment> = Vec::new();
    for assignment in &mut next.assignments {
        if assignment.state != AssignmentState::Running
            || !expired.contains(&assignment.id)
            || replaced.contains(&assignment.id)
        {
            continue;
        }
        assignment.state = AssignmentState::Ended;
        assignment.ended_at = Some(now_ms);
        assignment.ended_reason = Some(HOST_RESTARTED.to_owned());
        let read_up_to = assignment.report.as_ref().map_or(0, |report| report.seq);
        let note = RecoveryNote {
            replaces: assignment.id.clone(),
            effects: effects
                .iter()
                .filter(|effect| effect.assignment_id == assignment.id)
                .map(|effect| RecoveredEffect {
                    description: effect.description.clone(),
                    receipt: if effect.receipt.is_some() {
                        ReceiptState::Confirmed
                    } else {
                        ReceiptState::Uncertain
                    },
                })
                .collect(),
            moved_inputs: delivery
                .inputs
                .iter()
                .filter(|input| input.seq > read_up_to)
                .map(|input| input.seq)
                .collect(),
        };
        let run_trace_id = mint_run_trace(&assignment.id);
        opened.push(DeliveryAssignment {
            schema_version: DELIVERY_SCHEMA_VERSION,
            id: format!(
                "assignment:{}",
                &sha256_hex(format!("{}\n{run_trace_id}", delivery.id).as_bytes())[..32]
            ),
            run_trace_id,
            state: AssignmentState::Pending,
            report: None,
            started_at: None,
            ended_at: None,
            ended_reason: None,
            recovery: Some(note),
        });
    }
    next.assignments.extend(opened);
    next
}
