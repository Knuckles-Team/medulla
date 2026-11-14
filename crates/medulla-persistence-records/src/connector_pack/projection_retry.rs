//! MEDULLA-TYPED-PACKS-R096.2: per-pack projection retry state.
//!
//! The import outbox is ordered: a row that stays leased holds every later
//! row behind it. So the projection worker resolves every row in the sweep
//! that claims it, and a retryable projection failure moves THAT pack into
//! its own durable retry state on its head row instead: the head record, the
//! attempt count, the next-attempt time and the last refusal code. Retries
//! run from that state. When the attempts run out the state becomes a
//! recorded terminal failure that keeps its code; the head stays not visible
//! and `AttachPack` refuses with that code at once.
//!
//! The lifecycle is `PackProjectionMachine` in `state-machines-v1.ttl`;
//! [`PACK_PROJECTION_MACHINE`] is the same edge table and [`next_retry`] is the
//! only function that produces a new retry state.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// Where one head record's projection stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionPhase {
    /// Queued on the import outbox; not yet attempted.
    Pending,
    /// A retryable failure; another attempt is scheduled.
    Retrying,
    /// The projection applied; the head is visible.
    Applied,
    /// Terminal: the attempts ran out or the failure is not retryable.
    Failed,
}

impl MachineState for ProjectionPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Retrying => "retrying",
            Self::Applied => "applied",
            Self::Failed => "failed",
        }
    }
}

/// The closed event set of `PackProjectionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionEvent {
    /// The projection committed.
    Apply,
    /// The projection failed with a retryable code.
    FailRetryable,
    /// The projection failed with a code that is not retryable.
    Refuse,
}

impl MachineEvent for ProjectionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Apply => "Apply",
            Self::FailRetryable => "FailRetryable",
            Self::Refuse => "Refuse",
        }
    }
}

/// The guard of a retryable failure: the attempt just made is below the
/// `connector_pack.projection_max_attempts` knob.
pub const ATTEMPTS_LEFT: &str = "attempts_left";

/// `PackProjectionMachine`: the closed edge table. Every other pair is refused.
pub static PACK_PROJECTION_MACHINE: MachineDef<ProjectionPhase, ProjectionEvent, &'static str> =
    MachineDef {
        name: "PackProjectionMachine",
        states: &[
            ProjectionPhase::Pending,
            ProjectionPhase::Retrying,
            ProjectionPhase::Applied,
            ProjectionPhase::Failed,
        ],
        initial: ProjectionPhase::Pending,
        terminals: &[ProjectionPhase::Applied, ProjectionPhase::Failed],
        events: &[
            ProjectionEvent::Apply,
            ProjectionEvent::FailRetryable,
            ProjectionEvent::Refuse,
        ],
        guards: &[ATTEMPTS_LEFT],
        edges: &[
            Edge {
                id: "P01",
                from: ProjectionPhase::Pending,
                event: ProjectionEvent::Apply,
                guard: None,
                to: ProjectionPhase::Applied,
            },
            Edge {
                id: "P02",
                from: ProjectionPhase::Pending,
                event: ProjectionEvent::FailRetryable,
                guard: Some(ATTEMPTS_LEFT),
                to: ProjectionPhase::Retrying,
            },
            Edge {
                id: "P03",
                from: ProjectionPhase::Pending,
                event: ProjectionEvent::FailRetryable,
                guard: None,
                to: ProjectionPhase::Failed,
            },
            Edge {
                id: "P04",
                from: ProjectionPhase::Pending,
                event: ProjectionEvent::Refuse,
                guard: None,
                to: ProjectionPhase::Failed,
            },
            Edge {
                id: "P05",
                from: ProjectionPhase::Retrying,
                event: ProjectionEvent::Apply,
                guard: None,
                to: ProjectionPhase::Applied,
            },
            Edge {
                id: "P06",
                from: ProjectionPhase::Retrying,
                event: ProjectionEvent::FailRetryable,
                guard: Some(ATTEMPTS_LEFT),
                to: ProjectionPhase::Retrying,
            },
            Edge {
                id: "P07",
                from: ProjectionPhase::Retrying,
                event: ProjectionEvent::FailRetryable,
                guard: None,
                to: ProjectionPhase::Failed,
            },
            Edge {
                id: "P08",
                from: ProjectionPhase::Retrying,
                event: ProjectionEvent::Refuse,
                guard: None,
                to: ProjectionPhase::Failed,
            },
        ],
    };

impl ProjectionPhase {
    /// The one transition function of `PackProjectionMachine`.
    pub fn apply(self, event: ProjectionEvent, attempts_left: bool) -> Result<Self, TransitionRefused> {
        transition(&PACK_PROJECTION_MACHINE, self, event, |guard| {
            guard == ATTEMPTS_LEFT && attempts_left
        })
        .map(|edge| edge.to)
    }
}

/// The durable retry state of one head record's projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackProjectionRetry {
    /// The head record the state belongs to; a newer head starts afresh.
    pub record_id: String,
    /// Attempts made so far.
    pub attempt: u32,
    /// When the next attempt is due; meaningless once terminal.
    pub next_attempt_ms: u64,
    /// The last refusal code.
    pub code: String,
    /// `retrying` or `failed`.
    pub phase: String,
}

impl PackProjectionRetry {
    /// Whether the state is the recorded terminal failure.
    pub fn is_terminal(&self) -> bool {
        self.phase == ProjectionPhase::Failed.stored()
    }
}

/// The retry knobs, read from the registry by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryKnobs {
    /// `connector_pack.projection_max_attempts`.
    pub max_attempts: u32,
    /// `connector_pack.projection_backoff_ms`: the first delay; each later
    /// attempt doubles it.
    pub backoff_ms: u64,
}

/// The retry state after one more failure of `record_id`'s projection with
/// `code`. `prior` is the state the head holds now; a state of another record
/// does not count.
pub fn next_retry(
    prior: Option<&PackProjectionRetry>,
    record_id: &str,
    code: &str,
    retryable: bool,
    now_ms: u64,
    knobs: RetryKnobs,
) -> Result<PackProjectionRetry, TransitionRefused> {
    let prior = prior.filter(|state| state.record_id == record_id);
    let (from, made) = match prior {
        Some(state) if state.is_terminal() => (ProjectionPhase::Failed, state.attempt),
        Some(state) => (ProjectionPhase::Retrying, state.attempt),
        None => (ProjectionPhase::Pending, 0),
    };
    let attempt = made.saturating_add(1);
    let event = if retryable {
        ProjectionEvent::FailRetryable
    } else {
        ProjectionEvent::Refuse
    };
    let to = from.apply(event, attempt < knobs.max_attempts)?;
    let shift = attempt.saturating_sub(1).min(16);
    let delay = knobs.backoff_ms.saturating_mul(1u64 << shift);
    Ok(PackProjectionRetry {
        record_id: record_id.to_string(),
        attempt,
        next_attempt_ms: now_ms.saturating_add(delay),
        code: code.to_string(),
        phase: to.stored().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOBS: RetryKnobs = RetryKnobs {
        max_attempts: 3,
        backoff_ms: 100,
    };

    // spec: MEDULLA-TYPED-PACKS-R096.2
    #[test]
    fn retries_back_off_and_end_in_a_terminal_failure_that_keeps_its_code() {
        let first = next_retry(None, "r1", "PACK_PROJECTION_BLOB_UNAVAILABLE", true, 1000, KNOBS)
            .unwrap();
        assert_eq!((first.attempt, first.next_attempt_ms), (1, 1100));
        assert!(!first.is_terminal());
        let second =
            next_retry(Some(&first), "r1", "PACK_PROJECTION_STORE_BUSY", true, 2000, KNOBS).unwrap();
        assert_eq!((second.attempt, second.next_attempt_ms), (2, 2200));
        let third =
            next_retry(Some(&second), "r1", "PACK_PROJECTION_STORE_BUSY", true, 3000, KNOBS).unwrap();
        assert_eq!(third.attempt, 3);
        assert!(third.is_terminal(), "the knob's attempts are used up");
        assert_eq!(third.code, "PACK_PROJECTION_STORE_BUSY");
        // A terminal failure accepts no further event.
        assert!(next_retry(Some(&third), "r1", "X", true, 4000, KNOBS).is_err());
        // A newer head record starts afresh.
        let fresh = next_retry(Some(&third), "r2", "X", true, 4000, KNOBS).unwrap();
        assert_eq!(fresh.attempt, 1);
        // A failure that is not retryable is terminal at once.
        assert!(next_retry(None, "r3", "BODY_MISSING", false, 0, KNOBS)
            .unwrap()
            .is_terminal());
    }

    #[test]
    fn the_edge_table_is_closed() {
        assert_eq!(PACK_PROJECTION_MACHINE.edges.len(), 8);
        for edge in PACK_PROJECTION_MACHINE.edges {
            assert!(PACK_PROJECTION_MACHINE.states.contains(&edge.from));
            assert!(PACK_PROJECTION_MACHINE.states.contains(&edge.to));
        }
        assert!(ProjectionPhase::Applied
            .apply(ProjectionEvent::Apply, true)
            .is_err());
    }

    const MACHINES: &str =
        include_str!("../../../medulla-core/ontology/state-machines-v1.ttl");

    fn title(stored: &str) -> String {
        format!("{}{}", stored[..1].to_uppercase(), &stored[1..])
    }

    // spec: MEDULLA-TYPED-PACKS-R096.2
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":PackProjectionMachine a :StateMachine"));
        for state in PACK_PROJECTION_MACHINE.states {
            assert!(MACHINES.contains(&format!(
                ":PackProjectionMachine_{} a",
                title(state.stored())
            )));
        }
        for edge in PACK_PROJECTION_MACHINE.edges {
            let block = format!(":PackProjectionMachine_{} a :PackProjectionTransition", edge.id);
            let start = MACHINES.find(&block).expect("edge declared");
            let text = &MACHINES[start..];
            let text = &text[..text.find(" .\n").unwrap()];
            assert!(text.contains(&format!(
                ":fromState :PackProjectionMachine_{}",
                title(edge.from.stored())
            )));
            assert!(text.contains(&format!(
                ":toState :PackProjectionMachine_{}",
                title(edge.to.stored())
            )));
            assert!(text.contains(&format!(
                ":firedBy :PackProjectionMachine_Event_{}",
                edge.event.name()
            )));
            assert_eq!(
                text.contains(":guardedBy :PackProjectionMachine_Guard_AttemptsLeft"),
                edge.guard.is_some(),
                "{}",
                edge.id
            );
        }
    }
}
