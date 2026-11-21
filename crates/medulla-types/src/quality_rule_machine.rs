//! MEDULLA-DATAPLAT-VOCAB-R009.6: the change-request state of a quality rule as one explicit
//! state machine (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` kind [`RuleStateKind`]; the approver a record carries beside
//! its state stays on the record and is set at the transition. [`RuleStateKind::apply`] is the
//! one transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `QualityRuleMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `QualityRuleMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStateKind {
    /// `proposed`.
    Proposed,
    /// `approved`.
    Approved,
}

/// The closed event set of `QualityRuleMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualityRuleEvent {
    /// A steward approves the rule.
    Approve,
}

impl MachineState for RuleStateKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
        }
    }
}

impl MachineEvent for QualityRuleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
        }
    }
}

/// `QualityRuleMachine`: the closed edge table. Every other pair is refused.
pub static QUALITY_RULE_MACHINE: MachineDef<RuleStateKind, QualityRuleEvent, &'static str> =
    MachineDef {
        name: "QualityRuleMachine",
        states: &[RuleStateKind::Proposed, RuleStateKind::Approved],
        initial: RuleStateKind::Proposed,
        terminals: &[RuleStateKind::Approved],
        events: &[QualityRuleEvent::Approve],
        guards: &[],
        edges: &[Edge {
            id: "QR01",
            from: RuleStateKind::Proposed,
            event: QualityRuleEvent::Approve,
            guard: None,
            to: RuleStateKind::Approved,
        }],
    };

impl RuleStateKind {
    /// Every state, in declaration order.
    pub const ALL: [RuleStateKind; 2] = [RuleStateKind::Proposed, RuleStateKind::Approved];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        RuleStateKind::Proposed
    }

    /// The one transition function of `QualityRuleMachine`.
    pub fn apply(self, event: QualityRuleEvent) -> Result<Self, TransitionRefused> {
        transition(&QUALITY_RULE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<QualityRuleEvent> {
        QUALITY_RULE_MACHINE
            .edges
            .iter()
            .find(|edge| edge.from == self && edge.to == to)
            .map(|edge| edge.event)
    }

    /// Move to `to` along its edge, or refuse with `TRANSITION_REFUSED`.
    pub fn move_to(self, to: Self) -> Result<Self, TransitionRefused> {
        match self.event_to(to) {
            Some(event) => self.apply(event),
            None => Err(TransitionRefused {
                machine: QUALITY_RULE_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        QUALITY_RULE_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.6
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(QUALITY_RULE_MACHINE.edges.len(), 1);
        for edge in QUALITY_RULE_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(RuleStateKind::initial(), QUALITY_RULE_MACHINE.initial);
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.6
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &QUALITY_RULE_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "QualityRuleMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        for state in RuleStateKind::ALL {
            assert_eq!(RuleStateKind::from_stored(state.stored()), Some(state));
        }
        assert_eq!(RuleStateKind::from_stored("weather"), None);
        assert!(RuleStateKind::Approved
            .move_to(RuleStateKind::Proposed)
            .is_err());
    }

    /// The production code of the writers builds states through `RuleState`'s methods.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.6
    #[test]
    fn no_writer_sets_state_outside_apply() {
        for source in [
            include_str!("../../medulla-catalog-types/src/quality.rs"),
            include_str!("../../medulla-h-jobs/src/quality_rule.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            assert!(
                production.contains("RuleState::initial"),
                "the writer routes through the machine"
            );
            assert!(
                !production.contains("state: RuleState::Proposed"),
                "a writer sets the state outside apply"
            );
            assert!(
                !production.contains("RuleState::Approved {"),
                "a writer builds an approved state outside RuleState's methods"
            );
        }
    }
}
