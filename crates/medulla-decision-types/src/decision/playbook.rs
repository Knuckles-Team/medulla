//! MEDULLA-DECISION-ENGINE-R704.1: remediation playbook vocabulary -- the
//! triggers a playbook answers, the steps it proposes, the playbook itself and
//! the correlation of inbound events a trigger is matched against.
//!
//! A playbook is a library component (`AgentComponentKind::Playbook`); these
//! are its decoded shapes. Everything here is a proposal: a step that writes
//! names the governed action path and the engine never runs it.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;

/// Most steps one playbook may propose.
pub const MAX_PLAYBOOK_STEPS: usize = 32;
/// Most events one correlation may group.
pub const MAX_CORRELATED_EVENTS: usize = 1024;

/// What a playbook answers: the closed set of triggers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TriggerKind {
    ServiceDown,
    ServiceFlapping,
    ResourcePressure,
    TicketClass { name: String },
}

impl TriggerKind {
    /// The `trigger` attribute value of the `Playbook` component for this kind.
    pub fn attribute_value(&self) -> &'static str {
        match self {
            Self::ServiceDown => "service_down",
            Self::ServiceFlapping => "service_flapping",
            Self::ResourcePressure => "resource_pressure",
            Self::TicketClass { .. } => "ticket_class",
        }
    }
}

/// One proposed step. `writes` marks a step that changes something outside the
/// engine; it is carried out only through the governed action path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposedStep {
    pub step_id: String,
    pub action_ref: String,
    pub writes: bool,
}

/// A typed remediation playbook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Playbook {
    pub id: String,
    pub revision: u32,
    pub trigger: TriggerKind,
    pub steps: BoundedVec<ProposedStep, MAX_PLAYBOOK_STEPS>,
    pub evidence_class: EvidenceClass,
}

/// Inbound events grouped by one correlation `key` inside a `window_ms` window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EventCorrelation {
    pub key: String,
    pub window_ms: u64,
    /// Event ids in arrival order.
    pub events: BoundedVec<String, MAX_CORRELATED_EVENTS>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::PLAYBOOK_TRIGGERS;

    /// The wire form is fixed: field names, order and tag encoding.
    // spec: MEDULLA-DECISION-ENGINE-R704.1
    #[test]
    fn playbook_types_round_trip_with_golden_vector() {
        let playbook = Playbook {
            id: "playbook:restart-flapping".into(),
            revision: 2,
            trigger: TriggerKind::ServiceFlapping,
            steps: BoundedVec::new(vec![
                ProposedStep {
                    step_id: "s1".into(),
                    action_ref: "action:inspect".into(),
                    writes: false,
                },
                ProposedStep {
                    step_id: "s2".into(),
                    action_ref: "action:restart".into(),
                    writes: true,
                },
            ])
            .unwrap(),
            evidence_class: EvidenceClass::Observation,
        };
        let json = serde_json::to_string(&playbook).unwrap();
        assert_eq!(
            json,
            r#"{"id":"playbook:restart-flapping","revision":2,"trigger":{"kind":"service_flapping"},"steps":[{"step_id":"s1","action_ref":"action:inspect","writes":false},{"step_id":"s2","action_ref":"action:restart","writes":true}],"evidence_class":"observation"}"#
        );
        assert_eq!(serde_json::from_str::<Playbook>(&json).unwrap(), playbook);

        let ticket = TriggerKind::TicketClass {
            name: "access-request".into(),
        };
        let json = serde_json::to_string(&ticket).unwrap();
        assert_eq!(json, r#"{"kind":"ticket_class","name":"access-request"}"#);
        assert_eq!(serde_json::from_str::<TriggerKind>(&json).unwrap(), ticket);

        let correlation = EventCorrelation {
            key: "svc-a".into(),
            window_ms: 300_000,
            events: BoundedVec::new(vec!["e1".into(), "e2".into(), "e3".into()]).unwrap(),
        };
        let json = serde_json::to_string(&correlation).unwrap();
        assert_eq!(
            json,
            r#"{"key":"svc-a","window_ms":300000,"events":["e1","e2","e3"]}"#
        );
        assert_eq!(
            serde_json::from_str::<EventCorrelation>(&json).unwrap(),
            correlation
        );

        // The trigger set matches the library component's closed attribute set.
        let kinds = [
            TriggerKind::ServiceDown,
            TriggerKind::ServiceFlapping,
            TriggerKind::ResourcePressure,
            ticket,
        ];
        let values: Vec<&str> = kinds.iter().map(TriggerKind::attribute_value).collect();
        assert_eq!(values, PLAYBOOK_TRIGGERS);

        // Unknown triggers and unknown fields refuse; steps are bounded.
        assert!(serde_json::from_str::<TriggerKind>(r#"{"kind":"meltdown"}"#).is_err());
        assert!(serde_json::from_str::<ProposedStep>(
            r#"{"step_id":"s","action_ref":"a","writes":false,"x":1}"#
        )
        .is_err());
        let steps = |n: usize| {
            (0..n)
                .map(|i| ProposedStep {
                    step_id: format!("s{i}"),
                    action_ref: "a".into(),
                    writes: false,
                })
                .collect::<Vec<_>>()
        };
        assert!(
            BoundedVec::<ProposedStep, MAX_PLAYBOOK_STEPS>::new(steps(MAX_PLAYBOOK_STEPS)).is_ok()
        );
        assert!(
            BoundedVec::<ProposedStep, MAX_PLAYBOOK_STEPS>::new(steps(MAX_PLAYBOOK_STEPS + 1))
                .is_err()
        );
    }
}
