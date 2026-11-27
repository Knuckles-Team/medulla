//! MEDULLA-UNIFIED-DATA-PLANE-R014.2.3 -- the catalog probe of an attached source as one
//! explicit state machine.
//!
//! Registering a source is a synchronous record write. Probing it (reading its catalog) is a
//! separate asynchronous job, and this machine is that job's lifecycle. [`ProbeState::apply`]
//! is the one transition function and [`ProbeRecord`] is the only holder of the state: it is
//! one field, written only through `apply`. The baseline for `SCHEMA_DRIFT` is the schema
//! digest captured at the first successful probe.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

/// The state of a source's catalog probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProbeState {
    /// Registered; never probed.
    Registered,
    /// A probe job is reading the catalog.
    Probing,
    /// The last probe matched the baseline digest.
    Baselined,
    /// The last probe read a schema that differs from the baseline.
    Drifted,
    /// The last probe failed to read the catalog.
    ProbeFailed,
}

/// An event of `CatalogProbeMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProbeEvent {
    /// A probe job claimed the source.
    ProbeStarted,
    /// The catalog was read and its digest is the baseline (or equals it).
    ProbeSucceeded,
    /// The catalog read failed.
    ProbeErrored,
    /// The catalog was read and its digest differs from the baseline.
    SchemaChanged,
}

impl MachineState for ProbeState {
    fn stored(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Probing => "probing",
            Self::Baselined => "baselined",
            Self::Drifted => "drifted",
            Self::ProbeFailed => "probe_failed",
        }
    }
}

impl MachineEvent for ProbeEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ProbeStarted => "ProbeStarted",
            Self::ProbeSucceeded => "ProbeSucceeded",
            Self::ProbeErrored => "ProbeErrored",
            Self::SchemaChanged => "SchemaChanged",
        }
    }
}

/// `CatalogProbeMachine`: the closed edge table. Every other pair of state and event is refused.
pub static CATALOG_PROBE_MACHINE: MachineDef<ProbeState, ProbeEvent, &'static str> = MachineDef {
    name: "CatalogProbeMachine",
    states: &[
        ProbeState::Registered,
        ProbeState::Probing,
        ProbeState::Baselined,
        ProbeState::Drifted,
        ProbeState::ProbeFailed,
    ],
    initial: ProbeState::Registered,
    terminals: &[],
    events: &[
        ProbeEvent::ProbeStarted,
        ProbeEvent::ProbeSucceeded,
        ProbeEvent::ProbeErrored,
        ProbeEvent::SchemaChanged,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "P01",
            from: ProbeState::Registered,
            event: ProbeEvent::ProbeStarted,
            guard: None,
            to: ProbeState::Probing,
        },
        Edge {
            id: "P02",
            from: ProbeState::Probing,
            event: ProbeEvent::ProbeSucceeded,
            guard: None,
            to: ProbeState::Baselined,
        },
        Edge {
            id: "P03",
            from: ProbeState::Probing,
            event: ProbeEvent::ProbeErrored,
            guard: None,
            to: ProbeState::ProbeFailed,
        },
        Edge {
            id: "P04",
            from: ProbeState::Probing,
            event: ProbeEvent::SchemaChanged,
            guard: None,
            to: ProbeState::Drifted,
        },
        Edge {
            id: "P05",
            from: ProbeState::Baselined,
            event: ProbeEvent::ProbeStarted,
            guard: None,
            to: ProbeState::Probing,
        },
        Edge {
            id: "P06",
            from: ProbeState::Drifted,
            event: ProbeEvent::ProbeStarted,
            guard: None,
            to: ProbeState::Probing,
        },
        Edge {
            id: "P07",
            from: ProbeState::ProbeFailed,
            event: ProbeEvent::ProbeStarted,
            guard: None,
            to: ProbeState::Probing,
        },
    ],
};

impl ProbeState {
    /// The one transition function of `CatalogProbeMachine`.
    pub fn apply(self, event: ProbeEvent) -> Result<Self, TransitionRefused> {
        transition(&CATALOG_PROBE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for ProbeState {
    type Event = ProbeEvent;
    type Facts = ();
    fn apply(&self, event: ProbeEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        ProbeState::apply(*self, event)
    }
}

/// The probe record persisted on a source record: the state (one field) and the baseline
/// schema digest captured at the first successful probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeRecord {
    state: ProbeState,
    baseline: Option<u64>,
}

impl Default for ProbeRecord {
    fn default() -> Self {
        Self {
            state: CATALOG_PROBE_MACHINE.initial,
            baseline: None,
        }
    }
}

impl ProbeRecord {
    /// The current state.
    pub fn state(&self) -> ProbeState {
        self.state
    }

    /// The schema digest of the first successful probe, once there is one.
    pub fn baseline(&self) -> Option<u64> {
        self.baseline
    }

    fn apply(&mut self, event: ProbeEvent) -> Result<(), TransitionRefused> {
        self.state = self.state.apply(event)?;
        Ok(())
    }

    /// A probe job claims the source.
    pub fn start(&mut self) -> Result<(), TransitionRefused> {
        self.apply(ProbeEvent::ProbeStarted)
    }

    /// The probe read a catalog with schema digest `digest`. The first success captures the
    /// baseline; a later digest equal to it is `ProbeSucceeded`, any other is `SchemaChanged`.
    pub fn finish_read(&mut self, digest: u64) -> Result<ProbeState, TransitionRefused> {
        let event = match self.baseline {
            Some(baseline) if baseline != digest => ProbeEvent::SchemaChanged,
            _ => ProbeEvent::ProbeSucceeded,
        };
        self.apply(event)?;
        if event == ProbeEvent::ProbeSucceeded && self.baseline.is_none() {
            self.baseline = Some(digest);
        }
        Ok(self.state)
    }

    /// The probe could not read the catalog.
    pub fn finish_error(&mut self) -> Result<ProbeState, TransitionRefused> {
        self.apply(ProbeEvent::ProbeErrored)?;
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.3
    #[test]
    fn every_legal_transition_applies_and_every_illegal_one_is_refused() {
        let machine = &CATALOG_PROBE_MACHINE;
        assert_eq!(machine.edges.len(), 7);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
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
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.machine, "CatalogProbeMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 5 * 4 - 7);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.3
    #[test]
    fn the_baseline_is_the_first_successful_digest_and_drift_is_a_changed_digest() {
        let mut record = ProbeRecord::default();
        assert_eq!(record.state(), ProbeState::Registered);
        // A result before a probe started is refused and changes nothing.
        assert!(record.finish_read(1).is_err());
        assert_eq!(record.state(), ProbeState::Registered);
        assert_eq!(record.baseline(), None);

        record.start().unwrap();
        assert_eq!(record.finish_error(), Ok(ProbeState::ProbeFailed));
        assert_eq!(record.baseline(), None);

        record.start().unwrap();
        assert_eq!(record.finish_read(7), Ok(ProbeState::Baselined));
        assert_eq!(record.baseline(), Some(7));

        record.start().unwrap();
        assert_eq!(record.finish_read(8), Ok(ProbeState::Drifted));
        assert_eq!(record.baseline(), Some(7), "drift never moves the baseline");

        record.start().unwrap();
        assert_eq!(record.finish_read(7), Ok(ProbeState::Baselined));
        // Two probes cannot run on one source.
        record.start().unwrap();
        assert!(record.start().is_err());
        assert_eq!(record.state(), ProbeState::Probing);
    }

    /// Lines that assign the state field outside `apply`.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.3
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("probe_machine.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains("self.state = "))
            .collect();
        assert_eq!(writes, vec!["self.state = self.state.apply(event)?;"]);
    }
}
