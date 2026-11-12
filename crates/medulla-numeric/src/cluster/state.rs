//! The run state machine, progress reports and cancellation (MEDULLA-CLUSTER-R004).

use super::refusal::ClusterRefusal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a run is. `Converged`, `BudgetExhausted` and `Refused` are end states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Seeding,
    Sampling,
    Updating,
    Converged,
    BudgetExhausted,
    Refused(ClusterRefusal),
}

/// What happened to a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    SeedsReady,
    BatchDrawn,
    StepDone { converged: bool, budget_left: bool },
    Refuse(ClusterRefusal),
}

impl State {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Seeding => "Seeding",
            Self::Sampling => "Sampling",
            Self::Updating => "Updating",
            Self::Converged => "Converged",
            Self::BudgetExhausted => "BudgetExhausted",
            Self::Refused(_) => "Refused",
        }
    }

    pub const fn is_end(&self) -> bool {
        matches!(
            self,
            Self::Converged | Self::BudgetExhausted | Self::Refused(_)
        )
    }
}

impl Event {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::SeedsReady => "SeedsReady",
            Self::BatchDrawn => "BatchDrawn",
            Self::StepDone { .. } => "StepDone",
            Self::Refuse(_) => "Refuse",
        }
    }
}

/// The only writer of a run's state. Any pair outside the table is `StateIllegal`.
pub fn transition(from: &State, event: &Event) -> Result<State, ClusterRefusal> {
    let next = match (from, event) {
        (State::Seeding, Event::SeedsReady) => State::Sampling,
        (State::Sampling, Event::BatchDrawn) => State::Updating,
        (
            State::Updating,
            Event::StepDone {
                converged: true, ..
            },
        ) => State::Converged,
        (
            State::Updating,
            Event::StepDone {
                converged: false,
                budget_left: true,
            },
        ) => State::Sampling,
        (
            State::Updating,
            Event::StepDone {
                converged: false,
                budget_left: false,
            },
        ) => State::BudgetExhausted,
        (State::Seeding | State::Sampling | State::Updating, Event::Refuse(r)) => {
            State::Refused(r.clone())
        }
        _ => {
            return Err(ClusterRefusal::StateIllegal {
                from: from.name(),
                event: event.name(),
            })
        }
    };
    Ok(next)
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

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Apply one event through the table and report a state change.
    pub fn advance(&mut self, event: Event, step: u32, shift: f64) -> Result<(), ClusterRefusal> {
        let next = transition(&self.state, &event)?;
        let force = next.is_end() || self.state == State::Seeding;
        self.state = next;
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
        if let Ok(next) = transition(&self.state, &Event::Refuse(refusal.clone())) {
            self.state = next;
            self.report(0, 0.0, true);
        }
        refusal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states() -> Vec<State> {
        vec![
            State::Seeding,
            State::Sampling,
            State::Updating,
            State::Converged,
            State::BudgetExhausted,
            State::Refused(ClusterRefusal::Cancelled),
        ]
    }

    fn events() -> Vec<Event> {
        vec![
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
            Event::Refuse(ClusterRefusal::KInvalid),
        ]
    }

    /// The expected table: `Some(next state name)` for a legal pair, `None` otherwise.
    fn expected(s: &State, e: &Event) -> Option<&'static str> {
        match (s, e) {
            (State::Seeding, Event::SeedsReady) => Some("Sampling"),
            (State::Sampling, Event::BatchDrawn) => Some("Updating"),
            (
                State::Updating,
                Event::StepDone {
                    converged: true, ..
                },
            ) => Some("Converged"),
            (
                State::Updating,
                Event::StepDone {
                    converged: false,
                    budget_left: true,
                },
            ) => Some("Sampling"),
            (
                State::Updating,
                Event::StepDone {
                    converged: false,
                    budget_left: false,
                },
            ) => Some("BudgetExhausted"),
            (State::Seeding | State::Sampling | State::Updating, Event::Refuse(_)) => {
                Some("Refused")
            }
            _ => None,
        }
    }

    // spec: MEDULLA-CLUSTER-R004.1

    #[test]
    fn legal_transitions_follow_table() {
        let t = transition(&State::Seeding, &Event::SeedsReady).unwrap();
        assert_eq!(t, State::Sampling);
        let t = transition(&State::Updating, &Event::Refuse(ClusterRefusal::Cancelled)).unwrap();
        assert_eq!(t, State::Refused(ClusterRefusal::Cancelled));
    }

    // spec: MEDULLA-CLUSTER-R004.2

    #[test]
    fn every_state_event_pair_is_decided() {
        let mut legal = 0;
        for s in states() {
            for e in events() {
                match (transition(&s, &e), expected(&s, &e)) {
                    (Ok(next), Some(name)) => {
                        assert_eq!(next.name(), name, "{s:?} + {e:?}");
                        legal += 1;
                    }
                    (Err(ClusterRefusal::StateIllegal { from, event }), None) => {
                        assert_eq!((from, event), (s.name(), e.name()));
                    }
                    (got, want) => panic!("{s:?} + {e:?}: got {got:?}, table says {want:?}"),
                }
            }
        }
        assert_eq!(legal, 8);
        for s in states().into_iter().filter(State::is_end) {
            for e in events() {
                assert!(transition(&s, &e).is_err(), "end state {s:?} took {e:?}");
            }
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
        assert_eq!(*run.state(), State::Converged);
        // Reports: the first step and the end state; fast steps in between are throttled.
        let seen = rec.0.lock().unwrap().clone();
        assert_eq!(seen, vec!["Sampling", "Converged"]);
        assert!(run.advance(Event::BatchDrawn, 2, 0.0).is_err());
    }
}
