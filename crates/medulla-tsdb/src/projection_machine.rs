//! MEDULLA-UNIFIED-DATA-PLANE-R768.4: the time-series projection status as an explicit state
//! machine (organization constitution Article XXXI).
//!
//! `Missing` and `CatchingUp` are the read-time status of a series with no projection row; no
//! edge enters them and no writer ever stores them. A scoped append writes `Ready` (`Append`),
//! the modal commit path records that the served copy reached the source meta (`CatchUp`) or
//! that its projection write failed (`Degrade`). [`ProjectionStatus::apply`] is the only
//! function that produces a stored status, and a refused event is the typed
//! `TRANSITION_REFUSED`. The ontology description is `SeriesProjectionMachine` in
//! `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefusalReason,
    TransitionRefused,
};

/// The status of one series' projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStatus {
    /// No series meta and no projection row (read-time only).
    Missing,
    /// Series meta but no projection row (read-time only).
    CatchingUp,
    /// The served copy reached the source meta.
    Ready,
    /// The projection write failed.
    Degraded,
}

impl MachineState for ProjectionStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::CatchingUp => "catching_up",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
        }
    }
}

/// The closed event set of `SeriesProjectionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionEvent {
    /// A scoped append wrote points and the projection row in one write transaction.
    Append,
    /// The modal commit path recorded that the served copy reached the source meta.
    CatchUp,
    /// The modal commit path recorded a failed projection write.
    Degrade,
}

impl MachineEvent for ProjectionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Append => "Append",
            Self::CatchUp => "CatchUp",
            Self::Degrade => "Degrade",
        }
    }
}

use ProjectionEvent::{Append, CatchUp, Degrade};
use ProjectionStatus::{CatchingUp, Degraded, Missing, Ready};

/// `SeriesProjectionMachine`: the closed edge table. Every other pair is refused.
pub static SERIES_PROJECTION_MACHINE: MachineDef<ProjectionStatus, ProjectionEvent, &'static str> =
    MachineDef {
        name: "SeriesProjectionMachine",
        states: &[Missing, CatchingUp, Ready, Degraded],
        initial: Missing,
        terminals: &[],
        events: &[Append, CatchUp, Degrade],
        guards: &[],
        edges: &[
            Edge { id: "SP01", from: Missing, event: Append, guard: None, to: Ready },
            Edge { id: "SP02", from: CatchingUp, event: Append, guard: None, to: Ready },
            Edge { id: "SP03", from: Ready, event: Append, guard: None, to: Ready },
            Edge { id: "SP04", from: Degraded, event: Append, guard: None, to: Ready },
            Edge { id: "SP05", from: Missing, event: CatchUp, guard: None, to: Ready },
            Edge { id: "SP06", from: CatchingUp, event: CatchUp, guard: None, to: Ready },
            Edge { id: "SP07", from: Ready, event: CatchUp, guard: None, to: Ready },
            Edge { id: "SP08", from: Degraded, event: CatchUp, guard: None, to: Ready },
            Edge { id: "SP09", from: Missing, event: Degrade, guard: None, to: Degraded },
            Edge { id: "SP10", from: CatchingUp, event: Degrade, guard: None, to: Degraded },
            Edge { id: "SP11", from: Ready, event: Degrade, guard: None, to: Degraded },
            Edge { id: "SP12", from: Degraded, event: Degrade, guard: None, to: Degraded },
        ],
    };

impl ProjectionStatus {
    /// Every status, in declaration order.
    pub const ALL: [ProjectionStatus; 4] = [Missing, CatchingUp, Ready, Degraded];

    /// The initial status of the machine.
    pub fn initial() -> Self {
        SERIES_PROJECTION_MACHINE.initial
    }

    /// The one transition function of `SeriesProjectionMachine`.
    pub fn apply(self, event: ProjectionEvent) -> Result<Self, TransitionRefused> {
        transition(&SERIES_PROJECTION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the first edge from `self` to `to`, if there is one. No edge enters
    /// `Missing` or `CatchingUp`.
    pub fn event_to(self, to: Self) -> Option<ProjectionEvent> {
        SERIES_PROJECTION_MACHINE
            .edges_from(self)
            .find(|edge| edge.to == to)
            .map(|edge| edge.event)
    }

    /// Move to `to` along the edge that reaches it. `move_to(Missing)` and
    /// `move_to(CatchingUp)` refuse from every state with `TRANSITION_REFUSED`.
    pub fn move_to(self, to: Self) -> Result<Self, TransitionRefused> {
        match self.event_to(to) {
            Some(event) => self.apply(event),
            None => Err(TransitionRefused {
                machine: SERIES_PROJECTION_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("MoveTo({})", to.stored()),
                reason: TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The status a stored text names, or `None` for a word outside the machine.
    pub fn from_stored(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.stored() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.4
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(SERIES_PROJECTION_MACHINE.edges.len(), 12);
        assert_eq!(ProjectionStatus::initial(), Missing);
        for edge in SERIES_PROJECTION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "edge {}", edge.id);
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        // Every state and event has an edge, so the table leaves no illegal pair; the refused
        // moves are the targets no edge reaches.
        for state in ProjectionStatus::ALL {
            for target in [Missing, CatchingUp] {
                let refused = state.move_to(target).unwrap_err();
                assert_eq!(refused.machine, "SeriesProjectionMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
            assert_eq!(state.move_to(Ready), Ok(Ready));
            assert_eq!(state.move_to(Degraded), Ok(Degraded));
        }
        // The stored forms round-trip, and a word outside the machine is not a status.
        for status in ProjectionStatus::ALL {
            assert_eq!(ProjectionStatus::from_stored(status.stored()), Some(status));
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\\"{}\\"", status.stored()));
            assert_eq!(serde_json::from_str::<ProjectionStatus>(&json).unwrap(), status);
        }
        assert_eq!(ProjectionStatus::from_stored("healthy"), None);
        assert!(serde_json::from_str::<ProjectionStatus>("\\"healthy\\"").is_err());
    }

    /// No writer in `store.rs` sets the stored status except through `apply`.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.4
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("store.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("status: ProjectionStatus::Ready"));
        assert!(!production.contains("status: ProjectionStatus::Degraded"));
        assert!(production.matches(".apply(ProjectionEvent::").count() >= 4);
    }
}
