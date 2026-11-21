//! MEDULLA-IDENTITY-R724.1: the stage of one application's admission as an explicit state
//! machine (organization constitution Article XXXI).
//!
//! The name is `ApplicationAdmissionMachine` because `AdmissionMachine` already names the
//! per-write transaction machine. An admission starts `Pending`; `Admit`, `ConfirmParity` and
//! `RetireConnector` move it forward, and `RollBack` abandons it from `Admitted` or
//! `ParityConfirmed`. `ConnectorRetired` and `RolledBack` are terminal. [`AdmissionStage::apply`]
//! is the only function that produces a new stage; `ApplicationAdmission::advance` assigns its
//! result, and a refused event is the typed `TRANSITION_REFUSED`. The ontology description is
//! `ApplicationAdmissionMachine` in `state-machines-v1.ttl`.

use crate::platform_admission::AdmissionStage;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed event set of `ApplicationAdmissionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdmissionEvent {
    /// Attach the application to the shared platform.
    Admit,
    /// Confirm the attached copy is at parity with the prior connector.
    ConfirmParity,
    /// Retire the prior ingest connector.
    RetireConnector,
    /// Abandon the attachment.
    RollBack,
}

impl MachineEvent for AdmissionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Admit => "Admit",
            Self::ConfirmParity => "ConfirmParity",
            Self::RetireConnector => "RetireConnector",
            Self::RollBack => "RollBack",
        }
    }
}

impl MachineState for AdmissionStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Admitted => "admitted",
            Self::ParityConfirmed => "parity_confirmed",
            Self::ConnectorRetired => "connector_retired",
            Self::RolledBack => "rolled_back",
        }
    }
}

/// The admission machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationAdmissionGuard {}

/// `ApplicationAdmissionMachine`: the closed edge table. `ConnectorRetired` is reachable only
/// from `ParityConfirmed`. Every other pair of stage and event is refused.
pub static APPLICATION_ADMISSION_MACHINE: MachineDef<
    AdmissionStage,
    AdmissionEvent,
    ApplicationAdmissionGuard,
> = MachineDef {
    name: "ApplicationAdmissionMachine",
    states: &[
        AdmissionStage::Pending,
        AdmissionStage::Admitted,
        AdmissionStage::ParityConfirmed,
        AdmissionStage::ConnectorRetired,
        AdmissionStage::RolledBack,
    ],
    initial: AdmissionStage::Pending,
    terminals: &[AdmissionStage::ConnectorRetired, AdmissionStage::RolledBack],
    events: &[
        AdmissionEvent::Admit,
        AdmissionEvent::ConfirmParity,
        AdmissionEvent::RetireConnector,
        AdmissionEvent::RollBack,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "A01",
            from: AdmissionStage::Pending,
            event: AdmissionEvent::Admit,
            guard: None,
            to: AdmissionStage::Admitted,
        },
        Edge {
            id: "A02",
            from: AdmissionStage::Admitted,
            event: AdmissionEvent::ConfirmParity,
            guard: None,
            to: AdmissionStage::ParityConfirmed,
        },
        Edge {
            id: "A03",
            from: AdmissionStage::ParityConfirmed,
            event: AdmissionEvent::RetireConnector,
            guard: None,
            to: AdmissionStage::ConnectorRetired,
        },
        Edge {
            id: "A04",
            from: AdmissionStage::Admitted,
            event: AdmissionEvent::RollBack,
            guard: None,
            to: AdmissionStage::RolledBack,
        },
        Edge {
            id: "A05",
            from: AdmissionStage::ParityConfirmed,
            event: AdmissionEvent::RollBack,
            guard: None,
            to: AdmissionStage::RolledBack,
        },
    ],
};

impl AdmissionStage {
    /// The one transition function of `ApplicationAdmissionMachine`.
    pub fn apply(self, event: AdmissionEvent) -> Result<AdmissionStage, TransitionRefused> {
        transition(&APPLICATION_ADMISSION_MACHINE, self, event, |_| true).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-IDENTITY-R724.1
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(APPLICATION_ADMISSION_MACHINE.edges.len(), 5);
        for (id, from, event, to) in [
            ("A01", AdmissionStage::Pending, AdmissionEvent::Admit, AdmissionStage::Admitted),
            (
                "A02",
                AdmissionStage::Admitted,
                AdmissionEvent::ConfirmParity,
                AdmissionStage::ParityConfirmed,
            ),
            (
                "A03",
                AdmissionStage::ParityConfirmed,
                AdmissionEvent::RetireConnector,
                AdmissionStage::ConnectorRetired,
            ),
            ("A04", AdmissionStage::Admitted, AdmissionEvent::RollBack, AdmissionStage::RolledBack),
            (
                "A05",
                AdmissionStage::ParityConfirmed,
                AdmissionEvent::RollBack,
                AdmissionStage::RolledBack,
            ),
        ] {
            assert_eq!(from.apply(event), Ok(to), "edge {id}");
        }
        assert_eq!(AdmissionStage::Pending, APPLICATION_ADMISSION_MACHINE.initial);
    }

    // spec: MEDULLA-IDENTITY-R724.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &APPLICATION_ADMISSION_MACHINE;
        let mut refused_pairs = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "ApplicationAdmissionMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let reason = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, reason);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                refused_pairs += 1;
            }
        }
        // Five stages by four events, less the five edges.
        assert_eq!(refused_pairs, 15);
    }

    /// The only assignment to a stage is `ApplicationAdmission::advance`, from `apply`.
    // spec: MEDULLA-IDENTITY-R724.1
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sites = Vec::new();
        let mut files = vec![src];
        while let Some(path) = files.pop() {
            for entry in std::fs::read_dir(&path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let production = text.split("#[cfg(test)]").next().unwrap();
                    for line in production.lines().map(str::trim) {
                        if !line.starts_with("//") && line.contains(".stage = ") {
                            sites.push((path.clone(), line.to_string()));
                        }
                    }
                }
            }
        }
        assert_eq!(sites.len(), 1, "{sites:?}");
        assert!(sites[0].0.ends_with("platform_admission.rs"));
        assert_eq!(sites[0].1, "self.stage = self.stage.apply(event)?;");
    }
}
