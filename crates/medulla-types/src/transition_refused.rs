//! The core state-machine contract every medulla machine uses (MEDULLA-ORCH-VOCAB-R811,
//! organization constitution Article XXXI).
//!
//! A machine is data: a [`MachineDef`] names its closed state set, its terminal states and
//! its edge table. Each [`Edge`] carries an id, the state it leaves, the event that fires
//! it, an optional guard and the state it enters. [`transition`] is the one generic
//! transition function: every machine's own `apply` calls it with its definition, the
//! current state, the event and a guard evaluator. An illegal transition is refused with
//! one typed record, [`TransitionRefused`], whose wire code is `TRANSITION_REFUSED`.
//!
//! The ontology half of the same vocabulary lives in
//! `crates/medulla-core/ontology/state-machines-v1.ttl` (`:StateMachine`, `:State`,
//! `:TerminalState`, `:Transition`, `:TransitionEvent`, `:TransitionGuard`).

use serde::{Deserialize, Serialize};

use crate::contract::ServerErrorCode;

/// The wire code of every refused state-machine transition.
pub const TRANSITION_REFUSED: &str = "TRANSITION_REFUSED";

/// Every refusal code this module emits.
pub const REFUSALS: &[&str] = &[TRANSITION_REFUSED];

/// Why a machine refused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TransitionRefusalReason {
    /// The machine has edges for this event, but none leaves the current state.
    IllegalTransition,
    /// An edge leaves the current state on this event, but no such edge's guard holds.
    GuardFailed,
    /// The current state is terminal: it accepts no event.
    TerminalState,
    /// The machine declares no edge for this event at all.
    UnknownEvent,
}

impl TransitionRefusalReason {
    /// Every reason, in declaration order: the closed set.
    pub const ALL: [Self; 4] = [
        Self::IllegalTransition,
        Self::GuardFailed,
        Self::TerminalState,
        Self::UnknownEvent,
    ];

    /// The snake_case wire text of this reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IllegalTransition => "illegal_transition",
            Self::GuardFailed => "guard_failed",
            Self::TerminalState => "terminal_state",
            Self::UnknownEvent => "unknown_event",
        }
    }
}

/// The one typed refusal of an illegal transition. On the wire it is the object
/// `{code: "TRANSITION_REFUSED", machine, from, event, reason}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(try_from = "TransitionRefusedWire", into = "TransitionRefusedWire")]
#[error("TRANSITION_REFUSED: {machine} refused '{event}' from '{from}' ({})", reason.as_str())]
pub struct TransitionRefused {
    /// The machine instance name, such as `WorkItemMachine`.
    pub machine: String,
    /// The stored text of the state the machine was in.
    pub from: String,
    /// The event name.
    pub event: String,
    /// Why the event was refused.
    pub reason: TransitionRefusalReason,
}

impl TransitionRefused {
    /// The registered error code of this refusal.
    pub fn code(&self) -> ServerErrorCode {
        ServerErrorCode::TransitionRefused
    }
}

/// The wire form: the record plus its fixed `code` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TransitionRefusedWire {
    /// Always `TRANSITION_REFUSED`.
    pub code: String,
    /// The machine instance name.
    pub machine: String,
    /// The stored text of the current state.
    pub from: String,
    /// The event name.
    pub event: String,
    /// Why the event was refused.
    pub reason: TransitionRefusalReason,
}

impl From<TransitionRefused> for TransitionRefusedWire {
    fn from(value: TransitionRefused) -> Self {
        Self {
            code: TRANSITION_REFUSED.to_string(),
            machine: value.machine,
            from: value.from,
            event: value.event,
            reason: value.reason,
        }
    }
}

impl TryFrom<TransitionRefusedWire> for TransitionRefused {
    type Error = String;

    fn try_from(value: TransitionRefusedWire) -> Result<Self, Self::Error> {
        if value.code != TRANSITION_REFUSED {
            return Err(format!(
                "a transition refusal carries code '{TRANSITION_REFUSED}', not '{}'",
                value.code
            ));
        }
        Ok(Self {
            machine: value.machine,
            from: value.from,
            event: value.event,
            reason: value.reason,
        })
    }
}

/// A state of a machine: a closed, copyable value with one stored text.
pub trait MachineState: Copy + Eq + 'static {
    /// The stored text of this state (the `:storedForm` of its ontology instance).
    fn stored(self) -> &'static str;
}

/// An event of a machine: a copyable value with one name.
pub trait MachineEvent: Copy + Eq + 'static {
    /// The event name (the local name of its ontology instance).
    fn name(self) -> &'static str;
}

/// One edge of a machine's edge table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge<S: 'static, E: 'static, G: 'static> {
    /// The edge id, such as `W08`. The ontology transition is `:<Name>Machine_<id>`.
    pub id: &'static str,
    /// The state the edge leaves.
    pub from: S,
    /// The event that fires the edge.
    pub event: E,
    /// The guard that must hold, if any.
    pub guard: Option<G>,
    /// The state the edge enters.
    pub to: S,
}

/// A machine definition: a closed state set, its terminal subset, its initial state and
/// its edge table. Edges are evaluated in table order, so the first edge whose guard holds
/// fires.
#[derive(Debug, Clone, Copy)]
pub struct MachineDef<S: 'static, E: 'static, G: 'static> {
    /// The machine instance name, such as `WorkItemMachine`.
    pub name: &'static str,
    /// Every state, in declaration order.
    pub states: &'static [S],
    /// The one state a new record starts in.
    pub initial: S,
    /// The terminal states: they accept no event.
    pub terminals: &'static [S],
    /// Every event the machine knows.
    pub events: &'static [E],
    /// Every guard the machine names.
    pub guards: &'static [G],
    /// The edge table.
    pub edges: &'static [Edge<S, E, G>],
}

impl<S: MachineState, E: MachineEvent, G: Copy + Eq + 'static> MachineDef<S, E, G> {
    /// Whether `state` is terminal in this machine.
    pub fn is_terminal(&self, state: S) -> bool {
        self.terminals.contains(&state)
    }

    /// The state whose stored text is `stored`, if the machine declares one.
    pub fn state_from_stored(&self, stored: &str) -> Option<S> {
        self.states.iter().copied().find(|s| s.stored() == stored)
    }

    /// The edges that leave `from`, in table order.
    pub fn edges_from(&self, from: S) -> impl Iterator<Item = &'static Edge<S, E, G>> {
        self.edges.iter().filter(move |edge| edge.from == from)
    }

    fn refuse(&self, from: S, event: E, reason: TransitionRefusalReason) -> TransitionRefused {
        TransitionRefused {
            machine: self.name.to_string(),
            from: from.stored().to_string(),
            event: event.name().to_string(),
            reason,
        }
    }
}

/// The one generic transition function. Returns the edge that fires for `event` from
/// `from`, or the typed refusal. `guard_holds` evaluates a named guard; an edge with no
/// guard always holds.
pub fn transition<S, E, G>(
    def: &MachineDef<S, E, G>,
    from: S,
    event: E,
    guard_holds: impl Fn(G) -> bool,
) -> Result<&'static Edge<S, E, G>, TransitionRefused>
where
    S: MachineState,
    E: MachineEvent,
    G: Copy + Eq + 'static,
{
    if def.is_terminal(from) {
        return Err(def.refuse(from, event, TransitionRefusalReason::TerminalState));
    }
    if !def.edges.iter().any(|edge| edge.event == event) {
        return Err(def.refuse(from, event, TransitionRefusalReason::UnknownEvent));
    }
    let mut candidates = def
        .edges
        .iter()
        .filter(|edge| edge.from == from && edge.event == event)
        .peekable();
    if candidates.peek().is_none() {
        return Err(def.refuse(from, event, TransitionRefusalReason::IllegalTransition));
    }
    candidates
        .find(|edge| edge.guard.is_none_or(&guard_holds))
        .ok_or_else(|| def.refuse(from, event, TransitionRefusalReason::GuardFailed))
}

/// A machine whose state applies an event and returns the next state or the typed refusal.
pub trait StateMachineApply: Sized {
    /// The event type of the machine.
    type Event;
    /// The facts the guards read.
    type Facts;

    /// Apply `event` under `facts`: the next state, or the typed refusal.
    fn apply(&self, event: Self::Event, facts: &Self::Facts) -> Result<Self, TransitionRefused>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Lamp {
        Off,
        On,
        Broken,
    }
    impl MachineState for Lamp {
        fn stored(self) -> &'static str {
            match self {
                Self::Off => "off",
                Self::On => "on",
                Self::Broken => "broken",
            }
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Press {
        Toggle,
        Smash,
        Repair,
    }
    impl MachineEvent for Press {
        fn name(self) -> &'static str {
            match self {
                Self::Toggle => "toggle",
                Self::Smash => "smash",
                Self::Repair => "repair",
            }
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Powered {
        Yes,
    }
    const LAMP: MachineDef<Lamp, Press, Powered> = MachineDef {
        name: "LampMachine",
        states: &[Lamp::Off, Lamp::On, Lamp::Broken],
        initial: Lamp::Off,
        terminals: &[Lamp::Broken],
        events: &[Press::Toggle, Press::Smash, Press::Repair],
        guards: &[Powered::Yes],
        edges: &[
            Edge {
                id: "L01",
                from: Lamp::Off,
                event: Press::Toggle,
                guard: Some(Powered::Yes),
                to: Lamp::On,
            },
            Edge {
                id: "L02",
                from: Lamp::On,
                event: Press::Toggle,
                guard: None,
                to: Lamp::Off,
            },
            Edge {
                id: "L03",
                from: Lamp::On,
                event: Press::Smash,
                guard: None,
                to: Lamp::Broken,
            },
        ],
    };

    // spec: MEDULLA-ORCH-VOCAB-R811.4
    #[test]
    fn refusal_round_trips_and_code_is_unique() {
        let refused = TransitionRefused {
            machine: "WorkItemMachine".into(),
            from: "succeeded".into(),
            event: "cancel".into(),
            reason: TransitionRefusalReason::TerminalState,
        };
        let wire = serde_json::to_string(&refused).unwrap();
        let back: TransitionRefused = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, refused);
        assert_eq!(REFUSALS, &[TRANSITION_REFUSED]);
        assert_eq!(refused.code().as_str(), TRANSITION_REFUSED);
        // A record carrying another code is not a transition refusal.
        let foreign = wire.replace(TRANSITION_REFUSED, "SOMETHING_ELSE");
        assert!(serde_json::from_str::<TransitionRefused>(&foreign).is_err());
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.4
    #[test]
    fn wire_shape_has_code_and_four_fields() {
        let refused = TransitionRefused {
            machine: "WorkItemMachine".into(),
            from: "ready".into(),
            event: "start".into(),
            reason: TransitionRefusalReason::IllegalTransition,
        };
        let value = serde_json::to_value(&refused).unwrap();
        let object = value.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "event", "from", "machine", "reason"]);
        assert_eq!(object["code"], TRANSITION_REFUSED);
        assert_eq!(object["machine"], "WorkItemMachine");
        assert_eq!(object["from"], "ready");
        assert_eq!(object["event"], "start");
        assert_eq!(object["reason"], "illegal_transition");
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.4
    #[test]
    fn reason_serialises_snake_case_and_is_closed() {
        let texts: Vec<String> = TransitionRefusalReason::ALL
            .iter()
            .map(|reason| {
                serde_json::to_value(reason)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            texts,
            [
                "illegal_transition",
                "guard_failed",
                "terminal_state",
                "unknown_event"
            ]
        );
        for reason in TransitionRefusalReason::ALL {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }
        assert!(serde_json::from_str::<TransitionRefusalReason>("\"expired\"").is_err());
    }

    // spec: MEDULLA-ORCH-VOCAB-R811
    #[test]
    fn generic_transition_fires_edges_and_types_each_refusal() {
        let on = transition(&LAMP, Lamp::Off, Press::Toggle, |_| true).unwrap();
        assert_eq!((on.id, on.to), ("L01", Lamp::On));
        let refused = transition(&LAMP, Lamp::Off, Press::Toggle, |_| false).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        let refused = transition(&LAMP, Lamp::Off, Press::Smash, |_| true).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
        let refused = transition(&LAMP, Lamp::On, Press::Repair, |_| true).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::UnknownEvent);
        let refused = transition(&LAMP, Lamp::Broken, Press::Toggle, |_| true).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        assert_eq!(
            (
                refused.machine.as_str(),
                refused.from.as_str(),
                refused.event.as_str()
            ),
            ("LampMachine", "broken", "toggle")
        );
        assert_eq!(LAMP.state_from_stored("on"), Some(Lamp::On));
        assert_eq!(LAMP.state_from_stored("lit"), None);
    }
}
