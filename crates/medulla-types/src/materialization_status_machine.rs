//! The materialization status machine `MaterializationStatusMachine`
//! (MEDULLA-INCREMENTAL-VIEWS-R016.1, organization constitution Article XXXI).
//!
//! [`MaterializationStatus`] is the closed state set, [`MaterializationEvent`] the closed
//! event set, [`MATERIALIZATION_STATUS_MACHINE`] the edge table and
//! [`MaterializationStatus::apply`] the one transition function. The machine lives in this
//! leaf types crate so the durable reasoning projection and the in-memory truth-maintenance
//! index (`medulla-epistemic`) carry the same closed enum, and every writer of a
//! materialization status goes through one `fire` that calls `apply`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of the materialization status machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MaterializationStatus {
    /// The stored value matches its inputs.
    Fresh,
    /// An input, a policy scope or a generator changed since the value was computed.
    Stale,
    /// The materialization was deleted, or a recompute found no valid value.
    Retracted,
}

impl MachineState for MaterializationStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Retracted => "retracted",
        }
    }
}

/// The closed event set of the materialization status machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializationEvent {
    /// An input, a policy scope or a generator changed.
    Invalidate,
    /// The materialization itself was deleted, or a recompute found no valid value.
    Retract,
    /// The graph wrote provenance for an existing id again, or a recompute completed with
    /// provenance.
    Register,
}

impl MachineEvent for MaterializationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Invalidate => "Invalidate",
            Self::Retract => "Retract",
            Self::Register => "Register",
        }
    }
}

/// The materialization status machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializationGuard {}

/// The materialization status machine: `Fresh` start, no terminal state (a retracted id
/// becomes `Fresh` again on `Register`). Four pairs have no edge: re-registering a fresh
/// id, invalidating a stale id, and invalidating or retracting a retracted id. They are the
/// idempotent answers ([`MaterializationStatus::absorbs`]); writers answer them without
/// calling the machine.
pub const MATERIALIZATION_STATUS_MACHINE: MachineDef<
    MaterializationStatus,
    MaterializationEvent,
    MaterializationGuard,
> = MachineDef {
    name: "MaterializationStatusMachine",
    states: &[
        MaterializationStatus::Fresh,
        MaterializationStatus::Stale,
        MaterializationStatus::Retracted,
    ],
    initial: MaterializationStatus::Fresh,
    terminals: &[],
    events: &[
        MaterializationEvent::Invalidate,
        MaterializationEvent::Retract,
        MaterializationEvent::Register,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "M01",
            from: MaterializationStatus::Fresh,
            event: MaterializationEvent::Invalidate,
            guard: None,
            to: MaterializationStatus::Stale,
        },
        Edge {
            id: "M02",
            from: MaterializationStatus::Fresh,
            event: MaterializationEvent::Retract,
            guard: None,
            to: MaterializationStatus::Retracted,
        },
        Edge {
            id: "M03",
            from: MaterializationStatus::Stale,
            event: MaterializationEvent::Retract,
            guard: None,
            to: MaterializationStatus::Retracted,
        },
        Edge {
            id: "M04",
            from: MaterializationStatus::Stale,
            event: MaterializationEvent::Register,
            guard: None,
            to: MaterializationStatus::Fresh,
        },
        Edge {
            id: "M05",
            from: MaterializationStatus::Retracted,
            event: MaterializationEvent::Register,
            guard: None,
            to: MaterializationStatus::Fresh,
        },
    ],
};

impl MaterializationStatus {
    /// The one transition function: the next status, or the typed refusal.
    pub fn apply(self, event: MaterializationEvent) -> Result<Self, TransitionRefused> {
        transition(&MATERIALIZATION_STATUS_MACHINE, self, event, |guard| match guard {})
            .map(|edge| edge.to)
    }

    /// Whether `event` in this status is an idempotent no-op rather than a transition:
    /// exactly the four pairs without an edge.
    pub fn absorbs(self, event: MaterializationEvent) -> bool {
        matches!(
            (self, event),
            (Self::Fresh, MaterializationEvent::Register)
                | (Self::Stale, MaterializationEvent::Invalidate)
                | (Self::Retracted, MaterializationEvent::Invalidate)
                | (Self::Retracted, MaterializationEvent::Retract)
        )
    }

    /// The text a served result renders the status as.
    pub fn result_text(self) -> &'static str {
        match self {
            Self::Fresh => "Fresh",
            Self::Stale => "Stale",
            Self::Retracted => "Retracted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // proves R016.1: The materialization status (Fresh, Stale, Retracted) is one explicit state ...
    // spec: MEDULLA-INCREMENTAL-VIEWS-R016.1
    #[test]
    fn every_legal_transition_applies() {
        let machine = &MATERIALIZATION_STATUS_MACHINE;
        assert_eq!(machine.edges.len(), 5);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert!(!edge.from.absorbs(edge.event), "{}", edge.id);
        }
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R016.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &MATERIALIZATION_STATUS_MACHINE;
        let mut illegal = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event)
                {
                    continue;
                }
                illegal += 1;
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.machine, "MaterializationStatusMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                // Each illegal pair is exactly an absorbed pair.
                assert!(state.absorbs(event), "{state:?} {event:?}");
            }
        }
        assert_eq!(illegal, 4);
        assert_eq!(MaterializationStatus::Retracted.result_text(), "Retracted");
    }
}
