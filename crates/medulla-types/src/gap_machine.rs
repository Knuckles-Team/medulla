//! MEDULLA-DECISION-ENGINE-R830.5 -- the settlement state of a loop gap as one explicit
//! state machine (organization constitution Article XXXI).
//!
//! A gap is filed `Open`, an offer moves it to `Offered`, and settling moves it to
//! `Settled`, which never reopens. [`GapState::apply`] is the one transition function: it
//! is the only code that moves the state and it returns the typed [`TransitionRefused`]
//! for every pair the edge table does not list. The enum lives in this leaf types crate so
//! the stored gap record (`medulla-decision-types`) carries the same closed state.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The settlement state of a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GapState {
    /// Filed, no offer yet.
    Open,
    /// An offer is out.
    Offered,
    /// Settled; never reopens.
    Settled,
}

impl MachineState for GapState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Offered => "offered",
            Self::Settled => "settled",
        }
    }
}

/// The closed event set of the gap machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapEvent {
    /// A work offer is made for the gap.
    Offer,
    /// The offered gap is settled.
    Settle,
}

impl MachineEvent for GapEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Offer => "Offer",
            Self::Settle => "Settle",
        }
    }
}

/// `GapMachine`: the closed edge table. Every other pair of state and event is refused.
pub const GAP_MACHINE: MachineDef<GapState, GapEvent, &'static str> = MachineDef {
    name: "GapMachine",
    states: &[GapState::Open, GapState::Offered, GapState::Settled],
    initial: GapState::Open,
    terminals: &[GapState::Settled],
    events: &[GapEvent::Offer, GapEvent::Settle],
    guards: &[],
    edges: &[
        Edge {
            id: "G01",
            from: GapState::Open,
            event: GapEvent::Offer,
            guard: None,
            to: GapState::Offered,
        },
        Edge {
            id: "G02",
            from: GapState::Offered,
            event: GapEvent::Settle,
            guard: None,
            to: GapState::Settled,
        },
    ],
};

impl GapState {
    /// The state a newly filed gap takes.
    pub fn initial() -> Self {
        GAP_MACHINE.initial
    }

    /// The one transition function of `GapMachine`.
    pub fn apply(self, event: GapEvent) -> Result<Self, TransitionRefused> {
        transition(&GAP_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DECISION-ENGINE-R830.5
    #[test]
    fn every_legal_transition_applies() {
        for edge in GAP_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(GapState::initial(), GapState::Open);
    }

    // spec: MEDULLA-DECISION-ENGINE-R830.5
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &GAP_MACHINE;
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
                let refused = state.apply(event).unwrap_err();
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "GapMachine");
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

    /// No production line assigns a gap state from a literal variant.
    // spec: MEDULLA-DECISION-ENGINE-R830.5
    #[test]
    fn no_writer_sets_state_outside_apply() {
        for source in [
            include_str!("../../medulla-statechart/src/loops.rs"),
            include_str!("../../medulla-h-decide/src/decide/loops.rs"),
            include_str!("../../medulla-decision-types/src/loops.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".state = GapState::"))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
