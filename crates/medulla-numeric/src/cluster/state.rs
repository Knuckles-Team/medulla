//! The run state machine, progress reports and cancellation (MEDULLA-CLUSTER-R004).
//!
//! `ClusterRunMachine` (MEDULLA-CLUSTER-R004.5, organization constitution Article XXXI) is an
//! explicit state machine. [`State`] is its closed set of states, [`Event`] its closed set of
//! events, [`CLUSTER_RUN_MACHINE`] its edge table, and [`State::apply`] the one transition
//! function: [`ClusterRun`] holds the state in one field and writes it only through that
//! function. An illegal pair is refused with the core `TransitionRefused`, which names the
//! machine, the current state and the event, and it changes nothing. The ontology
//! description is `ClusterRunMachine` in `state-machines-v1.ttl`.

use super::refusal::ClusterRefusal;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a run is. `Converged`, `BudgetExhausted` and `Refused` are end states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Seeding,
    Sampling,
    Updating,
    Converged,
    BudgetExhausted,
    Refused,
}

/// What happened to a run. `StepDone` carries the two facts its guards read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    SeedsReady,
    BatchDrawn,
    StepDone { converged: bool, budget_left: bool },
    Refuse,
}

/// The event names of the edge table: one per verb, with the facts removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    SeedsReady,
    BatchDrawn,
    StepDone,
    Refuse,
}

/// The guards of the `StepDone` edges. Exactly one holds for any pair of facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Guard {
    /// The step converged.
    Converged,
    /// The step did not converge and a step is left.
    BudgetLeft,
    /// The step did not converge and no step is left.
    BudgetSpent,
}

impl MachineState for State {
    fn stored(self) -> &'static str {
        match self {
            Self::Seeding => "seeding",
            Self::Sampling => "sampling",
            Self::Updating => "updating",
            Self::Converged => "converged",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Refused => "refused",
        }
    }
}

impl MachineEvent for EventKind {
    fn name(self) -> &'static str {
        match self {
            Self::SeedsReady => "SeedsReady",
            Self::BatchDrawn => "BatchDrawn",
            Self::StepDone => "StepDone",
            Self::Refuse => "Refuse",
        }
    }
}

impl Event {
    /// The name this event has in the edge table.
    pub const fn kind(self) -> EventKind {
        match self {
            Self::SeedsReady => EventKind::SeedsReady,
            Self::BatchDrawn => EventKind::BatchDrawn,
            Self::StepDone { .. } => EventKind::StepDone,
            Self::Refuse => EventKind::Refuse,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::SeedsReady => "SeedsReady",
            Self::BatchDrawn => "BatchDrawn",
            Self::StepDone { .. } => "StepDone",
            Self::Refuse => "Refuse",
        }
    }
}

impl Guard {
    fn holds(self, event: Event) -> bool {
        let Event::StepDone {
            converged,
            budget_left,
        } = event
        else {
            return false;
        };
        match self {
            Self::Converged => converged,
            Self::BudgetLeft => !converged && budget_left,
            Self::BudgetSpent => !converged && !budget_left,
        }
    }
}

/// `ClusterRunMachine`: the closed edge table. Every pair it does not list is refused.
pub static CLUSTER_RUN_MACHINE: MachineDef<State, EventKind, Guard> = MachineDef {
    name: "ClusterRunMachine",
    states: &[
        State::Seeding,
        State::Sampling,
        State::Updating,
        State::Converged,
        State::BudgetExhausted,
        State::Refused,
    ],
    initial: State::Seeding,
    terminals: &[State::Converged, State::BudgetExhausted, State::Refused],
    events: &[
        EventKind::SeedsReady,
        EventKind::BatchDrawn,
        EventKind::StepDone,
        EventKind::Refuse,
    ],
    guards: &[Guard::Converged, Guard::BudgetLeft, Guard::BudgetSpent],
    edges: &[
        Edge {
            id: "C01",
            from: State::Seeding,
            event: EventKind::SeedsReady,
            guard: None,
            to: State::Sampling,
        },
        Edge {
            id: "C02",
            from: State::Sampling,
            event: EventKind::BatchDrawn,
            guard: None,
            to: State::Updating,
        },
        Edge {
            id: "C03",
            from: State::Updating,
            event: EventKind::StepDone,
            guard: Some(Guard::Converged),
            to: State::Converged,
        },
        Edge {
            id: "C04",
            from: State::Updating,
            event: EventKind::StepDone,
            guard: Some(Guard::BudgetLeft),
            to: State::Sampling,
        },
        Edge {
            id: "C05",
            from: State::Updating,
            event: EventKind::StepDone,
            guard: Some(Guard::BudgetSpent),
            to: State::BudgetExhausted,
        },
        Edge {
            id: "C06",
            from: State::Seeding,
            event: EventKind::Refuse,
            guard: None,
            to: State::Refused,
        },
        Edge {
            id: "C07",
            from: State::Sampling,
            event: EventKind::Refuse,
            guard: None,
            to: State::Refused,
        },
        Edge {
            id: "C08",
            from: State::Updating,
            event: EventKind::Refuse,
            guard: None,
            to: State::Refused,
        },
    ],
};

impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Seeding => "Seeding",
            Self::Sampling => "Sampling",
            Self::Updating => "Updating",
            Self::Converged => "Converged",
            Self::BudgetExhausted => "BudgetExhausted",
            Self::Refused => "Refused",
        }
    }

    pub const fn is_end(self) -> bool {
        matches!(
            self,
            Self::Converged | Self::BudgetExhausted | Self::Refused
        )
    }

    /// The one transition function of `ClusterRunMachine`: the next state, or the typed
    /// refusal naming the machine, this state and the event.
    pub fn apply(self, event: Event) -> Result<State, TransitionRefused> {
        transition(&CLUSTER_RUN_MACHINE, self, event.kind(), |guard| {
            guard.holds(event)
        })
        .map(|edge| edge.to)
    }
}

/// One progress report.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub state: &'static str,
    pub step: u32,
    pub max_steps: u32,
    pub shift: f64,
    pub elapsed_ms: u64,
}

/// Receives progress. A sink must not block.
pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: &Progress);
}

/// A sink that drops every report.
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _progress: &Progress) {}
}

/// Cooperative cancellation, checked at step boundaries.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The reporting interval of progress within one state.
pub const REPORT_INTERVAL: Duration = Duration::from_millis(250);

/// A run in flight: its state, its clock and its sink.
pub struct ClusterRun<'a> {
    state: State,
    started: Instant,
    last_report: Option<Instant>,
    sink: &'a dyn ProgressSink,
    max_steps: u32,
}

impl<'a> ClusterRun<'a> {
    pub fn new(sink: &'a dyn ProgressSink, max_steps: u32) -> Self {
        Self {
            state: State::Seeding,
            started: Instant::now(),
            last_report: None,
            sink,
            max_steps,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The only writer of the state field: it stores what `State::apply` returns and
    /// nothing else. An illegal pair changes nothing.
    fn step_to(&mut self, event: Event) -> Result<State, TransitionRefused> {
        self.state = self.state.apply(event)?;
        Ok(self.state)
    }

    /// Apply one event through the table and report a state change.
    pub fn advance(&mut self, event: Event, step: u32, shift: f64) -> Result<(), ClusterRefusal> {
        let was = self.state;
        let now = self.step_to(event).map_err(ClusterRefusal::StateIllegal)?;
        let force = now.is_end() || was == State::Seeding;
        self.report(step, shift, force);
        Ok(())
    }

    /// Report at the first step and at each end state, else at most once per interval.
    pub fn report(&mut self, step: u32, shift: f64, force: bool) {
        let due = self
            .last_report
            .is_none_or(|t| t.elapsed() >= REPORT_INTERVAL);
        if !(force || due) {
            return;
        }
        self.last_report = Some(Instant::now());
        self.sink.report(&Progress {
            state: self.state.name(),
            step,
            max_steps: self.max_steps,
            shift,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        });
    }

    /// Refuse the run: the state becomes `Refused` and the refusal is returned.
    pub fn refuse(&mut self, refusal: ClusterRefusal) -> ClusterRefusal {
        if self.step_to(Event::Refuse).is_ok() {
            self.report(0, 0.0, true);
        }
        refusal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [State; 6] = [
        State::Seeding,
        State::Sampling,
        State::Updating,
        State::Converged,
        State::BudgetExhausted,
        State::Refused,
    ];

    const EVENTS: [Event; 6] = [
        Event::SeedsReady,
        Event::BatchDrawn,
        Event::StepDone {
            converged: true,
            budget_left: true,
        },
        Event::StepDone {
            converged: false,
            budget_left: true,
        },
        Event::StepDone {
            converged: false,
            budget_left: false,
        },
        Event::Refuse,
    ];

    /// The expected table: the next state for a legal pair, `None` otherwise.
    fn expected(s: State, e: Event) -> Option<State> {
        match (s, e) {
            (State::Seeding, Event::SeedsReady) => Some(State::Sampling),
            (State::Sampling, Event::BatchDrawn) => Some(State::Updating),
            (
                State::Updating,
                Event::StepDone {
                    converged: true, ..
                },
            ) => Some(State::Converged),
            (
                State::Updating,
                Event::StepDone {
                    converged: false,
                    budget_left: true,
                },
            ) => Some(State::Sampling),
            (
                State::Updating,
                Event::StepDone {
                    converged: false,
                    budget_left: false,
                },
            ) => Some(State::BudgetExhausted),
            (State::Seeding | State::Sampling | State::Updating, Event::Refuse) => {
                Some(State::Refused)
            }
            _ => None,
        }
    }

    // spec: MEDULLA-CLUSTER-R004.1

    #[test]
    fn legal_transitions_follow_table() {
        assert_eq!(State::Seeding.apply(Event::SeedsReady), Ok(State::Sampling));
        assert_eq!(State::Updating.apply(Event::Refuse), Ok(State::Refused));
        assert_eq!(CLUSTER_RUN_MACHINE.initial, State::Seeding);
        assert_eq!(CLUSTER_RUN_MACHINE.edges.len(), 8);
    }

    // spec: MEDULLA-CLUSTER-R004.2

    #[test]
    fn every_state_event_pair_is_decided() {
        let mut legal = 0;
        for s in STATES {
            for e in EVENTS {
                match (s.apply(e), expected(s, e)) {
                    (Ok(next), Some(want)) => {
                        assert_eq!(next, want, "{s:?} + {e:?}");
                        legal += 1;
                    }
                    (Err(refused), None) => {
                        assert_eq!(refused.from, s.stored());
                        assert_eq!(refused.event, e.name());
                    }
                    (got, want) => panic!("{s:?} + {e:?}: got {got:?}, table says {want:?}"),
                }
            }
        }
        assert_eq!(legal, 8);
    }

    // spec: MEDULLA-CLUSTER-R004.5

    #[test]
    fn every_legal_transition_applies() {
        let mut taken = 0;
        for s in STATES {
            for e in EVENTS {
                if let Some(want) = expected(s, e) {
                    assert_eq!(s.apply(e), Ok(want), "{s:?} + {e:?}");
                    taken += 1;
                }
            }
        }
        // One test row per edge of the table.
        assert_eq!(taken, CLUSTER_RUN_MACHINE.edges.len());
        for edge in CLUSTER_RUN_MACHINE.edges {
            let fires = EVENTS
                .iter()
                .filter(|e| e.kind() == edge.event && State::apply(edge.from, **e) == Ok(edge.to))
                .count();
            assert!(fires >= 1, "edge {} is never taken", edge.id);
        }
    }

    // spec: MEDULLA-CLUSTER-R004.5

    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        use medulla_types::transition_refused::TransitionRefusalReason;
        let mut refused_pairs = 0;
        for s in STATES {
            for e in EVENTS {
                if expected(s, e).is_some() {
                    continue;
                }
                let refused = s.apply(e).unwrap_err();
                assert_eq!(refused.machine, "ClusterRunMachine");
                assert_eq!(refused.from, s.stored());
                assert_eq!(refused.event, e.name());
                let reason = if s.is_end() {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, reason, "{s:?} + {e:?}");
                refused_pairs += 1;
            }
        }
        assert_eq!(refused_pairs, STATES.len() * EVENTS.len() - 8);
        // An illegal pair changes nothing: the run keeps its state and reports nothing.
        let mut run = ClusterRun::new(&NoProgress, 3);
        let error = run.advance(Event::BatchDrawn, 0, 0.0).unwrap_err();
        assert!(matches!(error, ClusterRefusal::StateIllegal(ref r)
            if r.machine == "ClusterRunMachine" && r.from == "seeding" && r.event == "BatchDrawn"));
        assert_eq!(run.state(), State::Seeding);
    }

    // spec: MEDULLA-CLUSTER-R004.5

    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("state.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains("self.state ="))
            .collect();
        assert_eq!(writes.len(), 1, "{writes:?}");
        assert!(writes[0].contains("self.state.apply("), "{writes:?}");
        // No other file writes a run's state.
        for file in ["train.rs", "mod.rs", "online.rs"] {
            let text = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/cluster")
                    .join(file),
            )
            .unwrap();
            assert!(!text.contains(".state ="), "{file} writes a run state");
        }
    }

    struct Rec(std::sync::Mutex<Vec<&'static str>>);
    impl ProgressSink for Rec {
        fn report(&self, p: &Progress) {
            if let Ok(mut v) = self.0.lock() {
                v.push(p.state);
            }
        }
    }

    // spec: MEDULLA-CLUSTER-R004

    #[test]
    fn run_visits_states_in_table_order() {
        let rec = Rec(std::sync::Mutex::new(Vec::new()));
        let mut run = ClusterRun::new(&rec, 3);
        run.advance(Event::SeedsReady, 0, 0.0).unwrap();
        for step in 0..2 {
            run.advance(Event::BatchDrawn, step, 0.5).unwrap();
            run.advance(
                Event::StepDone {
                    converged: step == 1,
                    budget_left: true,
                },
                step,
                0.5,
            )
            .unwrap();
        }
        assert_eq!(run.state(), State::Converged);
        // Reports: the first step and the end state; fast steps in between are throttled.
        let seen = rec.0.lock().unwrap().clone();
        assert_eq!(seen, vec!["Sampling", "Converged"]);
        assert!(run.advance(Event::BatchDrawn, 2, 0.0).is_err());
    }
}
