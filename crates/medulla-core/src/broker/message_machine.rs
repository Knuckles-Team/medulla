//! MEDULLA-CQ-ORCH-R810.1 — the broker message as one explicit state machine.
//!
//! `MessageState::apply` is the one transition function. The edge table is B01 to B08.
//! The broker writers (`broker.rs`, `broker/consume_scan.rs` and the graph-core claim and
//! reclaim paths) move to it in the cutover step of the row.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

// BrokerMessageMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// The state of one broker message. A stored `claimed` value always means a live lease: `LeaseLapse` moves an expired claim back to `pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// The `pending` state.
    Pending,
    /// The `claimed` state.
    Claimed,
    /// The `acked` state.
    Acked,
    /// The `expired` state.
    Expired,
    /// The `dead_lettered` state.
    DeadLettered,
}

/// An event of `BrokerMessageMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageEvent {
    /// The `Claim` event.
    Claim,
    /// The `Ack` event.
    Ack,
    /// The `NackRetry` event.
    NackRetry,
    /// The `NackDead` event.
    NackDead,
    /// The `LeaseLapse` event.
    LeaseLapse,
    /// The `Expire` event.
    Expire,
}

impl MachineState for MessageState {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Acked => "acked",
            Self::Expired => "expired",
            Self::DeadLettered => "dead_lettered",
        }
    }
}

impl MachineEvent for MessageEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Claim => "Claim",
            Self::Ack => "Ack",
            Self::NackRetry => "NackRetry",
            Self::NackDead => "NackDead",
            Self::LeaseLapse => "LeaseLapse",
            Self::Expire => "Expire",
        }
    }
}

/// Guard of `BrokerMessageMachine`: deliver_at has passed and the message has not expired.
pub const BROKER_MESSAGE_GUARD_DELIVER_AT_PASSED_NOT_EXPIRED: &str = "DeliverAtPassedNotExpired";
/// Guard of `BrokerMessageMachine`: the claimant matches and the lease is live.
pub const BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_LEASE_LIVE: &str = "ClaimantMatchesLeaseLive";
/// Guard of `BrokerMessageMachine`: the claimant matches and attempts are below the maximum.
pub const BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_BELOW_MAX: &str =
    "ClaimantMatchesAttemptsBelowMax";
/// Guard of `BrokerMessageMachine`: the claimant matches.
pub const BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES: &str = "ClaimantMatches";
/// Guard of `BrokerMessageMachine`: the claimant matches and attempts reached the maximum.
pub const BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_REACHED_MAX: &str =
    "ClaimantMatchesAttemptsReachedMax";
/// Guard of `BrokerMessageMachine`: lease_until has passed.
pub const BROKER_MESSAGE_GUARD_LEASE_UNTIL_PASSED: &str = "LeaseUntilPassed";
/// Guard of `BrokerMessageMachine`: expires_at has passed.
pub const BROKER_MESSAGE_GUARD_EXPIRES_AT_PASSED: &str = "ExpiresAtPassed";

/// `BrokerMessageMachine`: the closed edge table. Every other pair of state and event is refused.
pub static BROKER_MESSAGE_MACHINE: MachineDef<MessageState, MessageEvent, &'static str> =
    MachineDef {
        name: "BrokerMessageMachine",
        states: &[
            MessageState::Pending,
            MessageState::Claimed,
            MessageState::Acked,
            MessageState::Expired,
            MessageState::DeadLettered,
        ],
        initial: MessageState::Pending,
        terminals: &[
            MessageState::Acked,
            MessageState::Expired,
            MessageState::DeadLettered,
        ],
        events: &[
            MessageEvent::Claim,
            MessageEvent::Ack,
            MessageEvent::NackRetry,
            MessageEvent::NackDead,
            MessageEvent::LeaseLapse,
            MessageEvent::Expire,
        ],
        guards: &[
            BROKER_MESSAGE_GUARD_DELIVER_AT_PASSED_NOT_EXPIRED,
            BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_LEASE_LIVE,
            BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_BELOW_MAX,
            BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES,
            BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_REACHED_MAX,
            BROKER_MESSAGE_GUARD_LEASE_UNTIL_PASSED,
            BROKER_MESSAGE_GUARD_EXPIRES_AT_PASSED,
        ],
        edges: &[
            Edge {
                id: "B01",
                from: MessageState::Pending,
                event: MessageEvent::Claim,
                guard: Some(BROKER_MESSAGE_GUARD_DELIVER_AT_PASSED_NOT_EXPIRED),
                to: MessageState::Claimed,
            },
            Edge {
                id: "B02",
                from: MessageState::Claimed,
                event: MessageEvent::Ack,
                guard: Some(BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_LEASE_LIVE),
                to: MessageState::Acked,
            },
            Edge {
                id: "B03",
                from: MessageState::Claimed,
                event: MessageEvent::NackRetry,
                guard: Some(BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_BELOW_MAX),
                to: MessageState::Pending,
            },
            Edge {
                id: "B04",
                from: MessageState::Claimed,
                event: MessageEvent::NackDead,
                guard: Some(BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES),
                to: MessageState::DeadLettered,
            },
            Edge {
                id: "B05",
                from: MessageState::Claimed,
                event: MessageEvent::NackRetry,
                guard: Some(BROKER_MESSAGE_GUARD_CLAIMANT_MATCHES_ATTEMPTS_REACHED_MAX),
                to: MessageState::DeadLettered,
            },
            Edge {
                id: "B06",
                from: MessageState::Claimed,
                event: MessageEvent::LeaseLapse,
                guard: Some(BROKER_MESSAGE_GUARD_LEASE_UNTIL_PASSED),
                to: MessageState::Pending,
            },
            Edge {
                id: "B07",
                from: MessageState::Pending,
                event: MessageEvent::Expire,
                guard: Some(BROKER_MESSAGE_GUARD_EXPIRES_AT_PASSED),
                to: MessageState::Expired,
            },
            Edge {
                id: "B08",
                from: MessageState::Claimed,
                event: MessageEvent::Expire,
                guard: Some(BROKER_MESSAGE_GUARD_EXPIRES_AT_PASSED),
                to: MessageState::Expired,
            },
        ],
    };

impl MessageState {
    /// The one transition function of `BrokerMessageMachine`. `guard_holds` answers each guard
    /// by name; an unguarded edge always holds.
    pub fn apply(
        self,
        event: MessageEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&BROKER_MESSAGE_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageEvent, MessageState, BROKER_MESSAGE_MACHINE};
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    fn edge_case(id: &str, from: MessageState, event: MessageEvent, to: MessageState) {
        let edge = BROKER_MESSAGE_MACHINE
            .edges
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        // Only this edge's guard holds, so a sibling edge on the same event cannot fire.
        assert_eq!(from.apply(event, |g| Some(g) == edge.guard), Ok(to));
        if edge.guard.is_some() {
            let refused = from.apply(event, |_| false).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        }
    }

    /// Every (state, event) pair with no edge.
    fn illegal_pairs() -> Vec<(MessageState, MessageEvent)> {
        let machine = &BROKER_MESSAGE_MACHINE;
        let mut out = Vec::new();
        for &state in machine.states {
            for &event in machine.events {
                if !machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    out.push((state, event));
                }
            }
        }
        out
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b01_pending_claim() {
        edge_case(
            "B01",
            MessageState::Pending,
            MessageEvent::Claim,
            MessageState::Claimed,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b02_claimed_ack() {
        edge_case(
            "B02",
            MessageState::Claimed,
            MessageEvent::Ack,
            MessageState::Acked,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b03_claimed_nack_retry() {
        edge_case(
            "B03",
            MessageState::Claimed,
            MessageEvent::NackRetry,
            MessageState::Pending,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b04_claimed_nack_dead() {
        edge_case(
            "B04",
            MessageState::Claimed,
            MessageEvent::NackDead,
            MessageState::DeadLettered,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b05_claimed_nack_retry() {
        edge_case(
            "B05",
            MessageState::Claimed,
            MessageEvent::NackRetry,
            MessageState::DeadLettered,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b06_claimed_lease_lapse() {
        edge_case(
            "B06",
            MessageState::Claimed,
            MessageEvent::LeaseLapse,
            MessageState::Pending,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b07_pending_expire() {
        edge_case(
            "B07",
            MessageState::Pending,
            MessageEvent::Expire,
            MessageState::Expired,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn edge_b08_claimed_expire() {
        edge_case(
            "B08",
            MessageState::Claimed,
            MessageEvent::Expire,
            MessageState::Expired,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &BROKER_MESSAGE_MACHINE;
        let pairs = illegal_pairs();
        assert!(!pairs.is_empty());
        for (state, event) in pairs {
            let refused = state.apply(event, |_| true).unwrap_err();
            let expected = if machine.is_terminal(state) {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, expected, "{state:?} {event:?}");
        }
        for &terminal in machine.terminals {
            for &event in machine.events {
                assert!(terminal.apply(event, |_| true).is_err());
            }
        }
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = MessageState::Pending
            .apply(MessageEvent::Ack, |_| true)
            .unwrap_err();
        assert_eq!(refused.machine, "BrokerMessageMachine");
        assert_eq!(refused.from, MessageState::Pending.stored());
        assert_eq!(refused.event, "Ack");
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
    }

    // spec: MEDULLA-CQ-ORCH-R810.1

    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &BROKER_MESSAGE_MACHINE;
        assert_eq!(machine.edges.len(), 8);
        assert_eq!(machine.states.len(), 5);
        let mut ids: Vec<_> = machine.edges.iter().map(|e| e.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), machine.edges.len(), "edge ids are unique");
        for edge in machine.edges {
            assert!(
                !machine.is_terminal(edge.from),
                "{} leaves a terminal state",
                edge.id
            );
        }
        for &state in machine.states {
            assert_eq!(machine.state_from_stored(state.stored()), Some(state));
            for &event in machine.events {
                let legal = state.apply(event, |_| true).is_ok();
                let has_edge = machine.edges_from(state).any(|e| e.event == event);
                assert_eq!(legal, has_edge, "{state:?} {event:?}");
            }
        }
    }
}
