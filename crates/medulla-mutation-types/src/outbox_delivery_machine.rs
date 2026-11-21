//! The outbox-delivery machine `OutboxDeliveryMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.13, organization constitution Article XXXI).
//!
//! [`OutboxDeliveryState`] is the closed state set, [`OutboxDeliveryEvent`] the closed
//! event set, [`OUTBOX_DELIVERY_MACHINE`] the edge table and
//! [`OutboxDeliveryState::apply`] the one transition function. A record starts `Pending`;
//! each delivery receipt applies one event to its predecessor's state. `Delivered` and
//! `FailedTerminal` are terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::outbox::OutboxDeliveryState;

/// The closed event set of the outbox-delivery machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxDeliveryEvent {
    /// A consumer leases the event.
    Lease,
    /// The leased delivery succeeded.
    Deliver,
    /// The leased delivery failed and may be retried.
    FailRetryable,
    /// The leased delivery failed for good.
    FailTerminal,
}

impl OutboxDeliveryEvent {
    /// The event that moves a receipt chain into `next`, or `None` for the record-only
    /// `Pending` state, which no receipt can reach.
    pub fn reaching(next: OutboxDeliveryState) -> Option<Self> {
        match next {
            OutboxDeliveryState::Pending => None,
            OutboxDeliveryState::Leased => Some(Self::Lease),
            OutboxDeliveryState::Delivered => Some(Self::Deliver),
            OutboxDeliveryState::FailedRetryable => Some(Self::FailRetryable),
            OutboxDeliveryState::FailedTerminal => Some(Self::FailTerminal),
        }
    }
}

impl MachineEvent for OutboxDeliveryEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Lease => "Lease",
            Self::Deliver => "Deliver",
            Self::FailRetryable => "FailRetryable",
            Self::FailTerminal => "FailTerminal",
        }
    }
}

impl MachineState for OutboxDeliveryState {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::Delivered => "delivered",
            Self::FailedRetryable => "failed_retryable",
            Self::FailedTerminal => "failed_terminal",
        }
    }
}

/// The guards of the outbox-delivery machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxDeliveryGuard {
    /// Attempt and lease epoch are unchanged from the predecessor receipt.
    SameAttempt,
    /// The attempt is one higher and the lease epoch is greater.
    NextAttempt,
}

/// The facts the guards read, taken from a receipt and its predecessor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutboxDeliveryFacts {
    /// [`OutboxDeliveryGuard::SameAttempt`] holds.
    pub same_attempt: bool,
    /// [`OutboxDeliveryGuard::NextAttempt`] holds.
    pub next_attempt: bool,
}

/// The outbox-delivery machine: `Pending` start; `Delivered` and `FailedTerminal` are
/// terminal.
pub const OUTBOX_DELIVERY_MACHINE: MachineDef<
    OutboxDeliveryState,
    OutboxDeliveryEvent,
    OutboxDeliveryGuard,
> = MachineDef {
    name: "OutboxDeliveryMachine",
    states: &[
        OutboxDeliveryState::Pending,
        OutboxDeliveryState::Leased,
        OutboxDeliveryState::Delivered,
        OutboxDeliveryState::FailedRetryable,
        OutboxDeliveryState::FailedTerminal,
    ],
    initial: OutboxDeliveryState::Pending,
    terminals: &[
        OutboxDeliveryState::Delivered,
        OutboxDeliveryState::FailedTerminal,
    ],
    events: &[
        OutboxDeliveryEvent::Lease,
        OutboxDeliveryEvent::Deliver,
        OutboxDeliveryEvent::FailRetryable,
        OutboxDeliveryEvent::FailTerminal,
    ],
    guards: &[
        OutboxDeliveryGuard::SameAttempt,
        OutboxDeliveryGuard::NextAttempt,
    ],
    edges: &[
        Edge {
            id: "O01",
            from: OutboxDeliveryState::Pending,
            event: OutboxDeliveryEvent::Lease,
            guard: None,
            to: OutboxDeliveryState::Leased,
        },
        Edge {
            id: "O02",
            from: OutboxDeliveryState::Leased,
            event: OutboxDeliveryEvent::Deliver,
            guard: Some(OutboxDeliveryGuard::SameAttempt),
            to: OutboxDeliveryState::Delivered,
        },
        Edge {
            id: "O03",
            from: OutboxDeliveryState::Leased,
            event: OutboxDeliveryEvent::FailRetryable,
            guard: Some(OutboxDeliveryGuard::SameAttempt),
            to: OutboxDeliveryState::FailedRetryable,
        },
        Edge {
            id: "O04",
            from: OutboxDeliveryState::Leased,
            event: OutboxDeliveryEvent::FailTerminal,
            guard: Some(OutboxDeliveryGuard::SameAttempt),
            to: OutboxDeliveryState::FailedTerminal,
        },
        Edge {
            id: "O05",
            from: OutboxDeliveryState::FailedRetryable,
            event: OutboxDeliveryEvent::Lease,
            guard: Some(OutboxDeliveryGuard::NextAttempt),
            to: OutboxDeliveryState::Leased,
        },
    ],
};

impl OutboxDeliveryState {
    /// The one transition function: the next state, or the typed refusal. `facts` is what
    /// the two guards read; the first receipt of a record carries none.
    pub fn apply(
        self,
        event: OutboxDeliveryEvent,
        facts: OutboxDeliveryFacts,
    ) -> Result<Self, TransitionRefused> {
        transition(&OUTBOX_DELIVERY_MACHINE, self, event, |guard| match guard {
            OutboxDeliveryGuard::SameAttempt => facts.same_attempt,
            OutboxDeliveryGuard::NextAttempt => facts.next_attempt,
        })
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    const HOLDING: OutboxDeliveryFacts = OutboxDeliveryFacts {
        same_attempt: true,
        next_attempt: true,
    };

    // spec: MEDULLA-DURABLE-KERNEL-R874.13
    #[test]
    fn every_legal_transition_applies() {
        for edge in OUTBOX_DELIVERY_MACHINE.edges {
            assert_eq!(
                edge.from.apply(edge.event, HOLDING),
                Ok(edge.to),
                "{}",
                edge.id
            );
        }
        // A guarded edge refuses when its guard does not hold.
        let failed = OutboxDeliveryState::Leased
            .apply(
                OutboxDeliveryEvent::Deliver,
                OutboxDeliveryFacts {
                    same_attempt: false,
                    next_attempt: true,
                },
            )
            .unwrap_err();
        assert_eq!(failed.reason, TransitionRefusalReason::GuardFailed);
        let failed = OutboxDeliveryState::FailedRetryable
            .apply(
                OutboxDeliveryEvent::Lease,
                OutboxDeliveryFacts {
                    same_attempt: true,
                    next_attempt: false,
                },
            )
            .unwrap_err();
        assert_eq!(failed.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.13
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &OUTBOX_DELIVERY_MACHINE;
        let mut illegal = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                illegal += 1;
                let refused = state.apply(event, HOLDING).unwrap_err();
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "OutboxDeliveryMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.13
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("outbox.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.contains(".state = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
        assert!(OutboxDeliveryEvent::reaching(OutboxDeliveryState::Pending).is_none());
    }
}
