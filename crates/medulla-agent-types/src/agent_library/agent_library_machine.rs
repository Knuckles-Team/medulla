//! MEDULLA-TYPED-PACKS-R810.2: the Agent Library lifecycle as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `Published` is the initial state. `Retired` is terminal (a durable tombstone). `Withdrawn`
//! is the reversible state only a connector pack member may enter. The closed event set is
//! `Publish` (a new revision of a published entry, or the republish of a withdrawn one),
//! `Retire` and `Withdraw`. [`AgentLibraryLifecycle::apply`] is the only function that
//! produces a new lifecycle; a refused event is the typed `TRANSITION_REFUSED` naming the
//! machine, the current state and the event, and changes nothing. The ontology description is
//! `AgentLibraryMachine` in `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefusalReason,
    TransitionRefused,
};

use super::lifecycle::{
    AgentLibraryLifecycle, LIFECYCLE_RETIRED_TERMINAL, LIFECYCLE_TRANSITION_INVALID,
    WITHDRAWN_NOT_ALLOWED,
};

/// The closed event set of `AgentLibraryMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentLibraryEvent {
    /// A new revision of a published entry, or the republish of a withdrawn one.
    Publish,
    /// The entry is tombstoned.
    Retire,
    /// A connector pack stops serving the entry.
    Withdraw,
}

/// The named guards of `AgentLibraryMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentLibraryGuard {
    /// The entry is a connector pack member (`mcp:` component id): only a pack importer can
    /// withdraw.
    PackMember,
}

/// The facts the guard reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleFacts {
    /// Whether the entry is a connector pack member.
    pub pack_member: bool,
}

impl LifecycleFacts {
    /// The facts of the component with this id: a pack member carries the `mcp:` prefix.
    pub fn of_component(component_id: &str) -> Self {
        Self {
            pack_member: component_id.starts_with("mcp:"),
        }
    }

    /// The facts of an entry that is not a pack member (agents, graphs, templates, library
    /// entries).
    pub fn native() -> Self {
        Self { pack_member: false }
    }
}

impl MachineState for AgentLibraryLifecycle {
    fn stored(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Retired => "retired",
            Self::Withdrawn => "withdrawn",
        }
    }
}

impl MachineEvent for AgentLibraryEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Publish => "Publish",
            Self::Retire => "Retire",
            Self::Withdraw => "Withdraw",
        }
    }
}

/// `AgentLibraryMachine`: the closed edge table. Every other pair is refused.
pub static AGENT_LIBRARY_MACHINE: MachineDef<
    AgentLibraryLifecycle,
    AgentLibraryEvent,
    AgentLibraryGuard,
> = MachineDef {
    name: "AgentLibraryMachine",
    states: &[
        AgentLibraryLifecycle::Published,
        AgentLibraryLifecycle::Retired,
        AgentLibraryLifecycle::Withdrawn,
    ],
    initial: AgentLibraryLifecycle::Published,
    terminals: &[AgentLibraryLifecycle::Retired],
    events: &[
        AgentLibraryEvent::Publish,
        AgentLibraryEvent::Retire,
        AgentLibraryEvent::Withdraw,
    ],
    guards: &[AgentLibraryGuard::PackMember],
    edges: &[
        Edge {
            id: "L01",
            from: AgentLibraryLifecycle::Published,
            event: AgentLibraryEvent::Publish,
            guard: None,
            to: AgentLibraryLifecycle::Published,
        },
        Edge {
            id: "L02",
            from: AgentLibraryLifecycle::Withdrawn,
            event: AgentLibraryEvent::Publish,
            guard: None,
            to: AgentLibraryLifecycle::Published,
        },
        Edge {
            id: "L03",
            from: AgentLibraryLifecycle::Published,
            event: AgentLibraryEvent::Retire,
            guard: None,
            to: AgentLibraryLifecycle::Retired,
        },
        Edge {
            id: "L04",
            from: AgentLibraryLifecycle::Published,
            event: AgentLibraryEvent::Withdraw,
            guard: Some(AgentLibraryGuard::PackMember),
            to: AgentLibraryLifecycle::Withdrawn,
        },
    ],
};

impl AgentLibraryLifecycle {
    /// The state a new entry takes: the machine's initial state.
    pub fn initial() -> Self {
        AGENT_LIBRARY_MACHINE.initial
    }

    /// The one transition function of `AgentLibraryMachine`.
    pub fn apply(
        self,
        event: AgentLibraryEvent,
        facts: &LifecycleFacts,
    ) -> Result<Self, TransitionRefused> {
        transition(&AGENT_LIBRARY_MACHINE, self, event, |guard| match guard {
            AgentLibraryGuard::PackMember => facts.pack_member,
        })
        .map(|edge| edge.to)
    }

    /// The event that moves an entry to `next`.
    pub fn event_to(next: Self) -> AgentLibraryEvent {
        match next {
            Self::Published => AgentLibraryEvent::Publish,
            Self::Retired => AgentLibraryEvent::Retire,
            Self::Withdrawn => AgentLibraryEvent::Withdraw,
        }
    }
}

/// The registered refusal code a lifecycle refusal carries on the wire for the Agent Library
/// callers (MEDULLA-DECISION-ENGINE-R168.4): a terminal `Retired` entry refuses with
/// `LIFECYCLE_RETIRED_TERMINAL`, a refused withdrawal guard with `WITHDRAWN_NOT_ALLOWED`, any
/// other illegal pair with `LIFECYCLE_TRANSITION_INVALID`. The text keeps the typed record
/// (machine, current state, event) after the code.
pub fn lifecycle_refusal_text(refused: &TransitionRefused) -> String {
    let code = match refused.reason {
        TransitionRefusalReason::TerminalState => LIFECYCLE_RETIRED_TERMINAL,
        TransitionRefusalReason::GuardFailed => WITHDRAWN_NOT_ALLOWED,
        TransitionRefusalReason::IllegalTransition | TransitionRefusalReason::UnknownEvent => {
            LIFECYCLE_TRANSITION_INVALID
        }
    };
    format!("{code}: {refused}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [AgentLibraryLifecycle; 3] = [
        AgentLibraryLifecycle::Published,
        AgentLibraryLifecycle::Retired,
        AgentLibraryLifecycle::Withdrawn,
    ];
    const EVENTS: [AgentLibraryEvent; 3] = [
        AgentLibraryEvent::Publish,
        AgentLibraryEvent::Retire,
        AgentLibraryEvent::Withdraw,
    ];
    const PACK: LifecycleFacts = LifecycleFacts { pack_member: true };

    // spec: MEDULLA-TYPED-PACKS-R810.2
    #[test]
    fn every_legal_transition_applies() {
        for edge in AGENT_LIBRARY_MACHINE.edges {
            assert_eq!(
                edge.from.apply(edge.event, &PACK),
                Ok(edge.to),
                "edge {}",
                edge.id
            );
        }
        assert_eq!(
            AgentLibraryLifecycle::initial(),
            AgentLibraryLifecycle::Published
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R810.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for from in STATES {
            for event in EVENTS {
                let legal = AGENT_LIBRARY_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == from && e.event == event);
                if legal {
                    continue;
                }
                let refused = from.apply(event, &PACK).unwrap_err();
                assert_eq!(refused.machine, "AgentLibraryMachine");
                assert_eq!(refused.from, from.stored());
                assert_eq!(refused.event, event.name());
                let expected = if from == AgentLibraryLifecycle::Retired {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected, "{from:?} {event:?}");
            }
        }
        // A native entry cannot be withdrawn: the guard fails.
        let refused = AgentLibraryLifecycle::Published
            .apply(AgentLibraryEvent::Withdraw, &LifecycleFacts::native())
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        assert!(lifecycle_refusal_text(&refused).starts_with(WITHDRAWN_NOT_ALLOWED));
    }

    // spec: MEDULLA-TYPED-PACKS-R810.2
    #[test]
    fn no_writer_sets_lifecycle_outside_apply() {
        // Production code never assigns a non-initial lifecycle directly: every move is the
        // result of `AgentLibraryLifecycle::apply`.
        let files = [
            ("agent_component.rs", include_str!("../agent_component.rs")),
            ("agent_graph.rs", include_str!("../agent_graph.rs")),
            ("agent_library.rs", include_str!("../agent_library.rs")),
            ("agent_template.rs", include_str!("../agent_template.rs")),
            (
                "connector_pack/admin.rs",
                include_str!("../../../medulla-persistence-records/src/connector_pack/admin.rs"),
            ),
            (
                "import/planning.rs",
                include_str!(
                    "../../../medulla-h-admin/src/admin/connector_pack/import/planning.rs"
                ),
            ),
        ];
        for (name, source) in files {
            let production = source.split("#[cfg(test)]").next().unwrap_or(source);
            for (n, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                let bare = code.trim();
                let forbidden = code.contains("lifecycle = AgentLibraryLifecycle::")
                    || code.contains("lifecycle: AgentLibraryLifecycle::Retired")
                    || code.contains("lifecycle: AgentLibraryLifecycle::Withdrawn")
                    || bare == "AgentLibraryLifecycle::Retired,"
                    || bare == "AgentLibraryLifecycle::Withdrawn,";
                assert!(
                    !forbidden,
                    "{name}:{}: sets a lifecycle outside apply",
                    n + 1
                );
            }
        }
    }
}
