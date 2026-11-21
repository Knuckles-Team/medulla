//! MEDULLA-SDLC-R019.7: the proposal machine of a delivery.

use medulla_source_types::delivery::{
    ProposalEvent, ProposalState, DELIVERY_PROPOSAL_MACHINE,
};
use medulla_types::transition_refused::{MachineEvent, MachineState, TransitionRefusalReason};

// spec: MEDULLA-SDLC-R019.7
#[test]
fn proposal_machine_refuses_every_pair_outside_the_table() {
    let machine = &DELIVERY_PROPOSAL_MACHINE;
    assert_eq!(machine.edges.len(), 3);
    assert_eq!(
        ProposalState::Open.apply(ProposalEvent::Merged),
        Ok(ProposalState::Merged)
    );
    assert_eq!(
        ProposalState::Open.apply(ProposalEvent::Closed),
        Ok(ProposalState::Closed)
    );
    assert_eq!(
        ProposalState::Closed.apply(ProposalEvent::Reopened),
        Ok(ProposalState::Open)
    );
    let mut refused_pairs = 0;
    for &state in machine.states {
        for &event in machine.events {
            if machine
                .edges
                .iter()
                .any(|e| e.from == state && e.event == event)
            {
                continue;
            }
            refused_pairs += 1;
            let refused = state.apply(event).unwrap_err();
            assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            assert_eq!(refused.machine, "DeliveryProposalMachine");
            assert_eq!(refused.from, state.stored());
            assert_eq!(refused.event, event.name());
            if state == ProposalState::Merged {
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
            }
        }
    }
    assert_eq!(refused_pairs, 3 * 3 - 3);
}
