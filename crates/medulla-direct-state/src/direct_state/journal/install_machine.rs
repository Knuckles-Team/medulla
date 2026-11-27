//! The direct-state install machine `DirectStateInstallMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.4, organization constitution Article XXXI).
//!
//! [`DirectStateInstallPhase`] is the closed state set, [`DirectStateInstallEvent`] the
//! closed event set, [`DIRECT_STATE_INSTALL_MACHINE`] the edge table and
//! [`DirectStateInstallPhase::apply`] the one transition function. An install journal is
//! written `Prepared`; publishing it is the only event, and `Published` is terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::DirectStateInstallPhase;

/// The closed event set of the install machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectStateInstallEvent {
    /// The Prepared journal is replaced by its Published successor.
    Publish,
}

impl MachineEvent for DirectStateInstallEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Publish => "Publish",
        }
    }
}

impl MachineState for DirectStateInstallPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Published => "published",
        }
    }
}

/// The install machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectStateInstallGuard {}

/// The install machine: `Prepared` start; `Published` is terminal.
pub const DIRECT_STATE_INSTALL_MACHINE: MachineDef<
    DirectStateInstallPhase,
    DirectStateInstallEvent,
    DirectStateInstallGuard,
> = MachineDef {
    name: "DirectStateInstallMachine",
    states: &[
        DirectStateInstallPhase::Prepared,
        DirectStateInstallPhase::Published,
    ],
    initial: DirectStateInstallPhase::Prepared,
    terminals: &[DirectStateInstallPhase::Published],
    events: &[DirectStateInstallEvent::Publish],
    guards: &[],
    edges: &[Edge {
        id: "D01",
        from: DirectStateInstallPhase::Prepared,
        event: DirectStateInstallEvent::Publish,
        guard: None,
        to: DirectStateInstallPhase::Published,
    }],
};

impl DirectStateInstallPhase {
    /// The one transition function: the next phase, or the typed refusal.
    pub fn apply(self, event: DirectStateInstallEvent) -> Result<Self, TransitionRefused> {
        transition(
            &DIRECT_STATE_INSTALL_MACHINE,
            self,
            event,
            |guard| match guard {},
        )
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.4
    #[test]
    fn every_legal_transition_applies() {
        for edge in DIRECT_STATE_INSTALL_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &DIRECT_STATE_INSTALL_MACHINE;
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
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                assert_eq!(refused.machine, "DirectStateInstallMachine");
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.4
    #[test]
    fn no_writer_sets_phase_outside_apply() {
        for (name, source) in [
            ("journal/install.rs", include_str!("install.rs")),
            ("registry/intake.rs", include_str!("../registry/intake.rs")),
            (
                "registry/publication.rs",
                include_str!("../registry/publication.rs"),
            ),
            (
                "registry/recovery.rs",
                include_str!("../registry/recovery.rs"),
            ),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with("//") && line.contains(".phase = "))
                .collect();
            assert!(writes.is_empty(), "{name}: {writes:?}");
        }
    }
}
