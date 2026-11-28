//! The machine lifecycle machine (MEDULLA-MACHINE-R005).
//!
//! `MachineLifecycleMachine` has five states, five events and seven edges `H01` to `H07`. It
//! follows the `CapacityLeaseMachine` pattern: one closed edge table, one transition function
//! and the typed refusal `TRANSITION_REFUSED`. The ontology mirror lives in
//! `crates/medulla-core/ontology/state-machines-v1.ttl`; a test asserts that it equals the
//! table. The evidence is six booleans; [`derive_event`] turns them into at most one event.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The stored property that holds the lifecycle state of a machine. Only [`MachineLifecycleState::apply`]
/// yields the value a writer stores under it.
pub const MACHINE_LIFECYCLE_STATE_PROPERTY: &str = "machineLifecycleState";

/// A state of `MachineLifecycleMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MachineLifecycleState {
    /// The machine is known and not yet serving: the one state a machine starts in.
    Commissioned,
    /// The machine is reachable, ready and serving.
    Active,
    /// The machine is being emptied of workloads.
    Draining,
    /// The machine holds no workload and awaits repair or retirement.
    Maintenance,
    /// The machine left the fleet. Terminal.
    Retired,
}

/// An event of `MachineLifecycleMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MachineLifecycleEvent {
    /// The machine is reachable and ready.
    Ready,
    /// A drain starts: requested, or the machine fails its health check.
    Drain,
    /// Every workload left the machine.
    Drained,
    /// The drain ends and the machine serves again.
    Resume,
    /// The machine leaves the fleet.
    Retire,
}

impl MachineLifecycleState {
    /// The state whose stored text is `stored`.
    pub fn from_stored(stored: &str) -> Option<Self> {
        MACHINE_LIFECYCLE_MACHINE.state_from_stored(stored)
    }
}

impl MachineLifecycleEvent {
    /// The event named `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        MACHINE_LIFECYCLE_MACHINE
            .events
            .iter()
            .copied()
            .find(|event| event.name() == name)
    }
}

impl MachineState for MachineLifecycleState {
    fn stored(self) -> &'static str {
        match self {
            Self::Commissioned => "commissioned",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Maintenance => "maintenance",
            Self::Retired => "retired",
        }
    }
}

impl MachineEvent for MachineLifecycleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Drain => "Drain",
            Self::Drained => "Drained",
            Self::Resume => "Resume",
            Self::Retire => "Retire",
        }
    }
}

/// Guard of `MachineLifecycleMachine`: reachable and ready, with no drain, health failure or
/// retirement pending (H01, H04, H05).
pub const MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY: &str = "ReachableAndReady";
/// Guard of `MachineLifecycleMachine`: a drain or a retirement is requested, or the health check
/// fails (H02).
pub const MACHINE_LIFECYCLE_GUARD_DRAIN_OR_HEALTH_FAILING: &str = "DrainOrHealthFailing";
/// Guard of `MachineLifecycleMachine`: every workload left while a drain, a health failure or a
/// retirement holds (H03).
pub const MACHINE_LIFECYCLE_GUARD_EVACUATED: &str = "EvacuatedWhileDrainOrFailing";
/// Guard of `MachineLifecycleMachine`: the operator asked to retire the machine (H06, H07).
pub const MACHINE_LIFECYCLE_GUARD_RETIRE_REQUESTED: &str = "RetireRequested";

/// The evidence the guards read. A caller states each fact it established; a fact it did not
/// establish is false, so every guard fails closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
pub struct MachineLifecycleFacts {
    /// The machine answered a probe.
    pub reachable: bool,
    /// The machine reports itself ready.
    pub ready: bool,
    /// A health check fails.
    pub health_failing: bool,
    /// An operator asked for a drain.
    pub drain_requested: bool,
    /// No workload runs on the machine.
    pub workloads_evacuated: bool,
    /// An operator asked to retire the machine.
    pub retire_requested: bool,
}

impl MachineLifecycleFacts {
    /// Whether the named guard holds under these facts. An unknown name does not hold.
    pub fn holds(&self, guard: &str) -> bool {
        let drain_or_failing = self.health_failing || self.drain_requested || self.retire_requested;
        match guard {
            MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY => {
                self.reachable && self.ready && !drain_or_failing
            }
            MACHINE_LIFECYCLE_GUARD_DRAIN_OR_HEALTH_FAILING => drain_or_failing,
            MACHINE_LIFECYCLE_GUARD_EVACUATED => self.workloads_evacuated && drain_or_failing,
            MACHINE_LIFECYCLE_GUARD_RETIRE_REQUESTED => self.retire_requested,
            _ => false,
        }
    }
}

/// `MachineLifecycleMachine`: the closed edge table. Every other pair of state and event is
/// refused.
pub static MACHINE_LIFECYCLE_MACHINE: MachineDef<
    MachineLifecycleState,
    MachineLifecycleEvent,
    &'static str,
> = MachineDef {
    name: "MachineLifecycleMachine",
    states: &[
        MachineLifecycleState::Commissioned,
        MachineLifecycleState::Active,
        MachineLifecycleState::Draining,
        MachineLifecycleState::Maintenance,
        MachineLifecycleState::Retired,
    ],
    initial: MachineLifecycleState::Commissioned,
    terminals: &[MachineLifecycleState::Retired],
    events: &[
        MachineLifecycleEvent::Ready,
        MachineLifecycleEvent::Drain,
        MachineLifecycleEvent::Drained,
        MachineLifecycleEvent::Resume,
        MachineLifecycleEvent::Retire,
    ],
    guards: &[
        MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY,
        MACHINE_LIFECYCLE_GUARD_DRAIN_OR_HEALTH_FAILING,
        MACHINE_LIFECYCLE_GUARD_EVACUATED,
        MACHINE_LIFECYCLE_GUARD_RETIRE_REQUESTED,
    ],
    edges: &[
        Edge {
            id: "H01",
            from: MachineLifecycleState::Commissioned,
            event: MachineLifecycleEvent::Ready,
            guard: Some(MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY),
            to: MachineLifecycleState::Active,
        },
        Edge {
            id: "H02",
            from: MachineLifecycleState::Active,
            event: MachineLifecycleEvent::Drain,
            guard: Some(MACHINE_LIFECYCLE_GUARD_DRAIN_OR_HEALTH_FAILING),
            to: MachineLifecycleState::Draining,
        },
        Edge {
            id: "H03",
            from: MachineLifecycleState::Draining,
            event: MachineLifecycleEvent::Drained,
            guard: Some(MACHINE_LIFECYCLE_GUARD_EVACUATED),
            to: MachineLifecycleState::Maintenance,
        },
        Edge {
            id: "H04",
            from: MachineLifecycleState::Draining,
            event: MachineLifecycleEvent::Resume,
            guard: Some(MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY),
            to: MachineLifecycleState::Active,
        },
        Edge {
            id: "H05",
            from: MachineLifecycleState::Maintenance,
            event: MachineLifecycleEvent::Ready,
            guard: Some(MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY),
            to: MachineLifecycleState::Active,
        },
        Edge {
            id: "H06",
            from: MachineLifecycleState::Maintenance,
            event: MachineLifecycleEvent::Retire,
            guard: Some(MACHINE_LIFECYCLE_GUARD_RETIRE_REQUESTED),
            to: MachineLifecycleState::Retired,
        },
        Edge {
            id: "H07",
            from: MachineLifecycleState::Commissioned,
            event: MachineLifecycleEvent::Retire,
            guard: Some(MACHINE_LIFECYCLE_GUARD_RETIRE_REQUESTED),
            to: MachineLifecycleState::Retired,
        },
    ],
};

impl MachineLifecycleState {
    /// The one transition function of `MachineLifecycleMachine`: the next state under `facts`,
    /// or `TRANSITION_REFUSED`. No code outside this method sets a lifecycle state.
    pub fn apply(
        self,
        event: MachineLifecycleEvent,
        facts: &MachineLifecycleFacts,
    ) -> Result<Self, TransitionRefused> {
        self.apply_with_edge(event, facts).map(|(next, _)| next)
    }

    /// [`Self::apply`] and the id of the edge that fired.
    pub fn apply_with_edge(
        self,
        event: MachineLifecycleEvent,
        facts: &MachineLifecycleFacts,
    ) -> Result<(Self, &'static str), TransitionRefused> {
        transition(&MACHINE_LIFECYCLE_MACHINE, self, event, |guard| {
            facts.holds(guard)
        })
        .map(|edge| (edge.to, edge.id))
    }
}

/// The one event the facts call for in `state`, or none. At most one event derives per call, so
/// a replay of one evidence set yields one step.
pub fn derive_event(
    state: MachineLifecycleState,
    facts: &MachineLifecycleFacts,
) -> Option<MachineLifecycleEvent> {
    use MachineLifecycleEvent as E;
    use MachineLifecycleState as S;
    let ready = facts.holds(MACHINE_LIFECYCLE_GUARD_REACHABLE_AND_READY);
    let drain = facts.holds(MACHINE_LIFECYCLE_GUARD_DRAIN_OR_HEALTH_FAILING);
    let evacuated = facts.holds(MACHINE_LIFECYCLE_GUARD_EVACUATED);
    match state {
        S::Commissioned | S::Maintenance if facts.retire_requested => Some(E::Retire),
        S::Commissioned | S::Maintenance => ready.then_some(E::Ready),
        S::Active => drain.then_some(E::Drain),
        S::Draining if evacuated => Some(E::Drained),
        S::Draining => ready.then_some(E::Resume),
        S::Retired => None,
    }
}

impl MachineLifecycleFacts {
    /// Every fact set: the 64 combinations of the six booleans.
    #[cfg(test)]
    fn all() -> impl Iterator<Item = Self> {
        (0u8..64).map(|bits| Self {
            reachable: bits & 1 != 0,
            ready: bits & 2 != 0,
            health_failing: bits & 4 != 0,
            drain_requested: bits & 8 != 0,
            workloads_evacuated: bits & 16 != 0,
            retire_requested: bits & 32 != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;
    use MachineLifecycleEvent as E;
    use MachineLifecycleState as S;

    fn facts(f: impl FnOnce(&mut MachineLifecycleFacts)) -> MachineLifecycleFacts {
        let mut facts = MachineLifecycleFacts::default();
        f(&mut facts);
        facts
    }

    fn up() -> MachineLifecycleFacts {
        facts(|f| {
            f.reachable = true;
            f.ready = true;
        })
    }

    fn edge(id: &str) -> &'static Edge<S, E, &'static str> {
        MACHINE_LIFECYCLE_MACHINE
            .edges
            .iter()
            .find(|edge| edge.id == id)
            .expect("the edge exists")
    }

    fn fires(id: &str, passing: MachineLifecycleFacts) {
        let edge = edge(id);
        assert_eq!(
            edge.from.apply_with_edge(edge.event, &passing).unwrap(),
            (edge.to, edge.id)
        );
        let refused = edge
            .from
            .apply(edge.event, &MachineLifecycleFacts::default())
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h01_commissioned_ready() {
        fires("H01", up());
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h02_active_drain() {
        fires("H02", facts(|f| f.drain_requested = true));
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h03_draining_drained() {
        fires(
            "H03",
            facts(|f| {
                f.workloads_evacuated = true;
                f.health_failing = true;
            }),
        );
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h04_draining_resume() {
        fires("H04", up());
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h05_maintenance_ready() {
        fires("H05", up());
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h06_maintenance_retire() {
        fires("H06", facts(|f| f.retire_requested = true));
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn edge_h07_commissioned_retire() {
        fires("H07", facts(|f| f.retire_requested = true));
    }

    // spec: MEDULLA-MACHINE-R005.1
    #[test]
    fn the_table_has_five_states_five_events_and_seven_edges() {
        let machine = &MACHINE_LIFECYCLE_MACHINE;
        assert_eq!(machine.name, "MachineLifecycleMachine");
        assert_eq!(machine.states.len(), 5);
        assert_eq!(machine.events.len(), 5);
        assert_eq!(machine.edges.len(), 7);
        assert_eq!(machine.initial, S::Commissioned);
        assert_eq!(machine.terminals, [S::Retired]);
        let stored: Vec<&str> = machine.states.iter().map(|s| s.stored()).collect();
        assert_eq!(
            stored,
            [
                "commissioned",
                "active",
                "draining",
                "maintenance",
                "retired"
            ]
        );
        assert_eq!(S::from_stored("draining"), Some(S::Draining));
        assert_eq!(S::from_stored("Draining"), None);
        assert_eq!(E::from_name("Drained"), Some(E::Drained));
        assert_eq!(E::from_name("drained"), None);
    }

    // spec: MEDULLA-MACHINE-R005.2
    #[test]
    fn derive_event_table_matches_the_row() {
        let ready = up();
        let drain = facts(|f| f.drain_requested = true);
        let retire = facts(|f| f.retire_requested = true);
        let evacuated = facts(|f| {
            f.drain_requested = true;
            f.workloads_evacuated = true;
        });
        let rows: [(S, MachineLifecycleFacts, Option<E>); 15] = [
            (S::Commissioned, ready, Some(E::Ready)),
            (S::Commissioned, retire, Some(E::Retire)),
            (S::Commissioned, MachineLifecycleFacts::default(), None),
            (S::Active, drain, Some(E::Drain)),
            (S::Active, ready, None),
            (S::Draining, evacuated, Some(E::Drained)),
            (S::Draining, ready, Some(E::Resume)),
            (S::Draining, drain, None),
            (S::Maintenance, ready, Some(E::Ready)),
            (S::Maintenance, retire, Some(E::Retire)),
            (S::Maintenance, MachineLifecycleFacts::default(), None),
            (S::Retired, ready, None),
            (S::Retired, retire, None),
            (S::Retired, evacuated, None),
            (
                S::Active,
                facts(|f| f.health_failing = true),
                Some(E::Drain),
            ),
        ];
        for (state, evidence, want) in rows {
            assert_eq!(
                derive_event(state, &evidence),
                want,
                "{state:?} {evidence:?}"
            );
        }
        // The derivation and the table agree on every state over all 64 fact sets: a derived
        // event fires an edge, and no event derives only when no edge of the state fires.
        for state in MACHINE_LIFECYCLE_MACHINE.states {
            for evidence in MachineLifecycleFacts::all() {
                let derived = derive_event(*state, &evidence);
                let firing: Vec<E> = MACHINE_LIFECYCLE_MACHINE
                    .edges_from(*state)
                    .filter(|edge| edge.guard.is_none_or(|g| evidence.holds(g)))
                    .map(|edge| edge.event)
                    .collect();
                match derived {
                    Some(event) => assert!(
                        state.apply(event, &evidence).is_ok(),
                        "{state:?} {event:?} {evidence:?}"
                    ),
                    None => assert!(firing.is_empty(), "{state:?} {evidence:?} {firing:?}"),
                }
            }
        }
    }

    // spec: MEDULLA-MACHINE-R005.2
    #[test]
    fn guards_of_one_state_are_mutually_exclusive_over_all_64_fact_sets() {
        assert_eq!(MachineLifecycleFacts::all().count(), 64);
        for state in MACHINE_LIFECYCLE_MACHINE.states {
            for evidence in MachineLifecycleFacts::all() {
                let holding = MACHINE_LIFECYCLE_MACHINE
                    .edges_from(*state)
                    .filter(|edge| edge.guard.is_none_or(|g| evidence.holds(g)))
                    .count();
                assert!(holding <= 1, "{state:?} {evidence:?}: {holding} edges fire");
            }
        }
        assert!(!MachineLifecycleFacts::default().holds("NoSuchGuard"));
        assert!(!up().holds("NoSuchGuard"));
    }

    // spec: MEDULLA-MACHINE-R005.3
    #[test]
    fn every_illegal_pair_is_refused() {
        let (mut legal, mut refused) = (0, 0);
        for state in MACHINE_LIFECYCLE_MACHINE.states {
            for event in MACHINE_LIFECYCLE_MACHINE.events {
                let passes = MachineLifecycleFacts::all().any(|f| state.apply(*event, &f).is_ok());
                if passes {
                    legal += 1;
                    continue;
                }
                refused += 1;
                let listed = MACHINE_LIFECYCLE_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == *state && edge.event == *event);
                let want = match (*state, listed) {
                    (S::Retired, _) => TransitionRefusalReason::TerminalState,
                    (_, true) => TransitionRefusalReason::GuardFailed,
                    (_, false) => TransitionRefusalReason::IllegalTransition,
                };
                for evidence in MachineLifecycleFacts::all() {
                    let error = state.apply(*event, &evidence).unwrap_err();
                    assert_eq!(error.reason, want, "{state:?} {event:?}");
                    assert_eq!(error.machine, "MachineLifecycleMachine");
                    assert!(error.to_string().starts_with("TRANSITION_REFUSED: "));
                }
            }
        }
        assert_eq!((legal, refused), (7, 18));
    }

    // spec: MEDULLA-MACHINE-R005.3
    #[test]
    fn an_active_machine_refuses_retire_and_a_retired_machine_refuses_every_event() {
        let retire = facts(|f| f.retire_requested = true);
        let error = S::Active.apply(E::Retire, &retire).unwrap_err();
        assert_eq!(error.reason, TransitionRefusalReason::IllegalTransition);
        for event in MACHINE_LIFECYCLE_MACHINE.events {
            let error = S::Retired.apply(*event, &up()).unwrap_err();
            assert_eq!(error.reason, TransitionRefusalReason::TerminalState);
        }
    }

    // spec: MEDULLA-MACHINE-R005.3
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut scanned = 0usize;
        let mut offenders = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if path.is_dir() {
                    if name != "target" && name != "target-isolated" && name != "tests" {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                scanned += 1;
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let production = text.split("#[cfg(test)]").next().unwrap_or_default();
                let literal = format!("\"{MACHINE_LIFECYCLE_STATE_PROPERTY}\"");
                if production.contains(&literal) && !path.ends_with("machine/lifecycle.rs") {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(scanned > 1000, "the scan reached only {scanned} files");
        assert!(
            offenders.is_empty(),
            "lifecycle state written outside apply: {offenders:?}"
        );
    }
}
