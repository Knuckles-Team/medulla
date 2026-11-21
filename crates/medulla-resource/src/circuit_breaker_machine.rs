//! MEDULLA-PLACEMENT-R784.1 — `CircuitBreakerMachine`, the endpoint breaker edge table.
//!
//! The machine runs over [`CircuitKind`], the payload-free kind of a
//! [`CircuitState`](crate::admission::CircuitState); the payload (`consecutive_failures`,
//! `since_ms`) stays in the stored value. [`next_state`] is the one place the edge table is
//! consulted, and `CircuitBreaker::apply` in `admission.rs` is the only code that assigns
//! `CircuitBreaker.state` after construction.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::admission::{CircuitState, Outcome};

/// The kind of a [`CircuitState`], with its payload dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CircuitKind {
    Closed,
    Open,
    HalfOpen,
}

impl MachineState for CircuitKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

impl CircuitState {
    /// The state's kind, without its payload.
    pub fn kind(self) -> CircuitKind {
        match self {
            Self::Closed { .. } => CircuitKind::Closed,
            Self::Open { .. } => CircuitKind::Open,
            Self::HalfOpen => CircuitKind::HalfOpen,
        }
    }
}

/// The event set of `CircuitBreakerMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CircuitEvent {
    /// The clock advanced; an open breaker may half-open.
    Tick,
    /// One call through the breaker was observed.
    Observe,
}

impl MachineEvent for CircuitEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Tick => "Tick",
            Self::Observe => "Observe",
        }
    }
}

/// An event with its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircuitStep {
    Tick { now_ms: u64 },
    Observe { outcome: Outcome, now_ms: u64 },
}

impl CircuitStep {
    /// The machine event of this step.
    pub fn event(self) -> CircuitEvent {
        match self {
            Self::Tick { .. } => CircuitEvent::Tick,
            Self::Observe { .. } => CircuitEvent::Observe,
        }
    }
}

/// The guards of `CircuitBreakerMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CircuitGuard {
    /// The open breaker's cooldown has passed.
    CooldownElapsed,
    /// The observed call succeeded.
    SuccessObserved,
    /// The observed call failed.
    FailureObserved,
    /// The observed failure leaves the consecutive count below the threshold.
    FailureBelowThreshold,
    /// The observed failure reaches the threshold.
    FailureThresholdReached,
}

/// `CircuitBreakerMachine`: the closed edge table. Every other pair of kind and event is
/// refused.
pub static CIRCUIT_BREAKER_MACHINE: MachineDef<CircuitKind, CircuitEvent, CircuitGuard> =
    MachineDef {
        name: "CircuitBreakerMachine",
        states: &[CircuitKind::Closed, CircuitKind::Open, CircuitKind::HalfOpen],
        initial: CircuitKind::Closed,
        terminals: &[],
        events: &[CircuitEvent::Tick, CircuitEvent::Observe],
        guards: &[
            CircuitGuard::CooldownElapsed,
            CircuitGuard::SuccessObserved,
            CircuitGuard::FailureObserved,
            CircuitGuard::FailureBelowThreshold,
            CircuitGuard::FailureThresholdReached,
        ],
        edges: &[
            // A success resets the failure count.
            Edge {
                id: "CB01",
                from: CircuitKind::Closed,
                event: CircuitEvent::Observe,
                guard: Some(CircuitGuard::SuccessObserved),
                to: CircuitKind::Closed,
            },
            Edge {
                id: "CB02",
                from: CircuitKind::Closed,
                event: CircuitEvent::Observe,
                guard: Some(CircuitGuard::FailureBelowThreshold),
                to: CircuitKind::Closed,
            },
            Edge {
                id: "CB03",
                from: CircuitKind::Closed,
                event: CircuitEvent::Observe,
                guard: Some(CircuitGuard::FailureThresholdReached),
                to: CircuitKind::Open,
            },
            // An open breaker ignores outcomes.
            Edge {
                id: "CB04",
                from: CircuitKind::Open,
                event: CircuitEvent::Observe,
                guard: None,
                to: CircuitKind::Open,
            },
            Edge {
                id: "CB05",
                from: CircuitKind::Open,
                event: CircuitEvent::Tick,
                guard: Some(CircuitGuard::CooldownElapsed),
                to: CircuitKind::HalfOpen,
            },
            Edge {
                id: "CB06",
                from: CircuitKind::HalfOpen,
                event: CircuitEvent::Observe,
                guard: Some(CircuitGuard::SuccessObserved),
                to: CircuitKind::Closed,
            },
            Edge {
                id: "CB07",
                from: CircuitKind::HalfOpen,
                event: CircuitEvent::Observe,
                guard: Some(CircuitGuard::FailureObserved),
                to: CircuitKind::Open,
            },
        ],
    };

/// The state a breaker in `state` enters on `step`, or the typed refusal. The guards read the
/// breaker's `failure_threshold` and `cooldown_ms` and the step; nothing is assigned here.
pub(crate) fn next_state(
    state: CircuitState,
    failure_threshold: u32,
    cooldown_ms: u64,
    step: CircuitStep,
) -> Result<CircuitState, TransitionRefused> {
    let (now_ms, outcome) = match step {
        CircuitStep::Tick { now_ms } => (now_ms, None),
        CircuitStep::Observe { outcome, now_ms } => (now_ms, Some(outcome)),
    };
    let failures = match state {
        CircuitState::Closed {
            consecutive_failures,
        } => consecutive_failures,
        _ => 0,
    };
    let since_ms = match state {
        CircuitState::Open { since_ms } => since_ms,
        _ => now_ms,
    };
    let guard_holds = |guard: CircuitGuard| match guard {
        CircuitGuard::CooldownElapsed => {
            matches!(state, CircuitState::Open { .. })
                && now_ms.saturating_sub(since_ms) >= cooldown_ms
        }
        CircuitGuard::SuccessObserved => outcome == Some(Outcome::Success),
        CircuitGuard::FailureObserved => outcome == Some(Outcome::Failure),
        CircuitGuard::FailureBelowThreshold => {
            outcome == Some(Outcome::Failure) && failures.saturating_add(1) < failure_threshold
        }
        CircuitGuard::FailureThresholdReached => {
            outcome == Some(Outcome::Failure) && failures.saturating_add(1) >= failure_threshold
        }
    };
    let edge = transition(&CIRCUIT_BREAKER_MACHINE, state.kind(), step.event(), guard_holds)?;
    Ok(match edge.to {
        CircuitKind::Closed => CircuitState::Closed {
            consecutive_failures: match edge.guard {
                Some(CircuitGuard::FailureBelowThreshold) => failures.saturating_add(1),
                _ => 0,
            },
        },
        CircuitKind::Open => CircuitState::Open {
            since_ms: match edge.from {
                CircuitKind::Open => since_ms,
                _ => now_ms,
            },
        },
        CircuitKind::HalfOpen => CircuitState::HalfOpen,
    })
}

#[cfg(test)]
mod tests {
    use medulla_types::transition_refused::TransitionRefusalReason;

    use super::*;
    use crate::admission::CircuitBreaker;

    fn breaker_in(state: CircuitState) -> CircuitBreaker {
        let mut breaker = CircuitBreaker::new(3, 100);
        breaker.state = state;
        breaker
    }

    // spec: MEDULLA-PLACEMENT-R784.1
    #[test]
    fn every_legal_transition_applies() {
        use CircuitState::{Closed, HalfOpen, Open};
        use CircuitStep::{Observe, Tick};
        use Outcome::{Failure, Success};
        let legal = [
            ("CB01", Closed { consecutive_failures: 2 }, Observe { outcome: Success, now_ms: 10 }, Closed { consecutive_failures: 0 }),
            ("CB02", Closed { consecutive_failures: 1 }, Observe { outcome: Failure, now_ms: 10 }, Closed { consecutive_failures: 2 }),
            ("CB03", Closed { consecutive_failures: 2 }, Observe { outcome: Failure, now_ms: 10 }, Open { since_ms: 10 }),
            ("CB04", Open { since_ms: 5 }, Observe { outcome: Failure, now_ms: 10 }, Open { since_ms: 5 }),
            ("CB05", Open { since_ms: 5 }, Tick { now_ms: 105 }, HalfOpen),
            ("CB06", HalfOpen, Observe { outcome: Success, now_ms: 10 }, Closed { consecutive_failures: 0 }),
            ("CB07", HalfOpen, Observe { outcome: Failure, now_ms: 10 }, Open { since_ms: 10 }),
        ];
        assert_eq!(legal.len(), CIRCUIT_BREAKER_MACHINE.edges.len());
        for (id, from, step, to) in legal {
            let edge = CIRCUIT_BREAKER_MACHINE
                .edges
                .iter()
                .find(|edge| edge.id == id)
                .unwrap_or_else(|| panic!("{id} is in the edge table"));
            assert_eq!(
                (edge.from, edge.event, edge.to),
                (from.kind(), step.event(), to.kind()),
                "{id}"
            );
            let mut breaker = breaker_in(from);
            assert_eq!(breaker.apply(step), Ok(to), "{id}");
            assert_eq!(breaker.state, to, "{id}");
        }
        assert_eq!(
            CIRCUIT_BREAKER_MACHINE.initial,
            CircuitBreaker::new(3, 100).state.kind()
        );
    }

    // spec: MEDULLA-PLACEMENT-R784.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        use CircuitState::{Closed, HalfOpen, Open};
        use CircuitStep::{Observe, Tick};
        let refused = [
            // The cooldown has not elapsed.
            (Open { since_ms: 5 }, Tick { now_ms: 50 }, TransitionRefusalReason::GuardFailed),
            // No edge leaves these kinds on a tick.
            (Closed { consecutive_failures: 0 }, Tick { now_ms: 500 }, TransitionRefusalReason::IllegalTransition),
            (HalfOpen, Tick { now_ms: 500 }, TransitionRefusalReason::IllegalTransition),
        ];
        for (state, step, reason) in refused {
            let mut breaker = breaker_in(state);
            let error = breaker.apply(step).expect_err("the pair is refused");
            assert_eq!(error.machine, "CircuitBreakerMachine");
            assert_eq!(error.from, state.kind().stored());
            assert_eq!(error.event, step.event().name());
            assert_eq!(error.reason, reason);
            assert_eq!(breaker.state, state, "a refusal changes nothing");
        }
        // Every pair the table gives no edge for is refused; the rest apply or are guarded.
        for &kind in CIRCUIT_BREAKER_MACHINE.states {
            for &event in CIRCUIT_BREAKER_MACHINE.events {
                let has_edge = CIRCUIT_BREAKER_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == kind && edge.event == event);
                let state = match kind {
                    CircuitKind::Closed => Closed { consecutive_failures: 0 },
                    CircuitKind::Open => Open { since_ms: 0 },
                    CircuitKind::HalfOpen => HalfOpen,
                };
                let step = match event {
                    CircuitEvent::Tick => Tick { now_ms: 0 },
                    CircuitEvent::Observe => Observe { outcome: Outcome::Success, now_ms: 0 },
                };
                if !has_edge {
                    let error = breaker_in(state).apply(step).expect_err("no edge");
                    assert_eq!(error.reason, TransitionRefusalReason::IllegalTransition);
                }
            }
        }
        // The observe writer maps the refusal to its coded error and keeps `None` refused as before.
        let mut breaker = CircuitBreaker::new(1, 10);
        assert!(breaker.observe(Some(Outcome::Failure), 1).is_ok());
        assert_eq!(breaker.state, Open { since_ms: 1 });
        breaker.tick(5);
        assert_eq!(breaker.state, Open { since_ms: 1 }, "an early tick is absorbed");
        breaker.tick(11);
        assert_eq!(breaker.state, HalfOpen);
        breaker.tick(500);
        assert_eq!(breaker.state, HalfOpen, "a tick of a half-open breaker is absorbed");
    }

    // spec: MEDULLA-PLACEMENT-R784.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/admission.rs"),
        )
        .unwrap();
        // Everything before the first test module is production code.
        let production = source.split("#[cfg(test)]").next().unwrap_or("");
        let apply_start = production
            .find("pub fn apply(&mut self, step: CircuitStep)")
            .expect("`CircuitBreaker::apply` is defined");
        let apply_end = apply_start
            + production[apply_start..]
                .find("\n    }\n")
                .expect("`apply` ends");
        let mut assignments = Vec::new();
        let mut offset = 0;
        for line in production.split_inclusive('\n') {
            if line.contains(".state = ") {
                assignments.push(offset);
            }
            offset += line.len();
        }
        assert_eq!(assignments.len(), 1, "state is assigned in one place: {assignments:?}");
        assert!(
            (apply_start..apply_end).contains(&assignments[0]),
            "the only assignment to `.state` is in `apply`"
        );
    }
}
