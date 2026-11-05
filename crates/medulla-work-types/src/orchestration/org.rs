//! Organization, market, twin and lifecycle records
//! (MEDULLA-ORCH-VOCAB-R750.750).
//!
//! The `Decide` families read these records through the immutable snapshot
//! of the organization store; instances move on `Method::Statechart`. Writes
//! go through the single write authority.
//!
//! The market defines no bid or allocation type of its own: the bid is the
//! [`WorkOffer`] / [`WorkOfferView`] / [`BlastRadius`] of
//! [`crate::work_market`], and the allocation inputs and output are its
//! [`MarketFacts`], [`RankedOffer`] and [`OfferSelection`].

use serde::{Deserialize, Serialize};

pub use crate::work_market::selection::{MarketFacts, OfferSelection, RankedOffer};
pub use crate::work_market::{BlastRadius, WorkOffer, WorkOfferView};

/// An organization and the roles it staffs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgChart {
    pub org_id: String,
    pub roles: Vec<RoleSpec>,
}

/// One role: the capabilities it needs and how many employees fill it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub role_id: String,
    pub required_capabilities: Vec<String>,
    pub headcount: u32,
}

/// An agent definition revision employed in one role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmployeeSpec {
    pub agent_ref: String,
    pub definition_revision: String,
    pub role_id: String,
}

/// One planned item of work for a role, ordered by its dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgPlanItem {
    pub item_id: String,
    pub role_id: String,
    pub depends_on: Vec<String>,
}

/// Refusal raised when an [`AgentTwin`] carries no version pin.
pub const TWIN_PIN_MISSING: &str = "TWIN_PIN_MISSING";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentTwinWire {
    agent_ref: String,
    pinned_revisions: Vec<String>,
}

/// A replay twin of an agent, pinned to the revisions it replays against.
/// A twin holds at least one pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AgentTwinWire")]
pub struct AgentTwin {
    agent_ref: String,
    pinned_revisions: Vec<String>,
}

impl AgentTwin {
    /// Build a twin; refuses with [`TWIN_PIN_MISSING`] when no pin is given.
    pub fn new(
        agent_ref: impl Into<String>,
        pinned_revisions: Vec<String>,
    ) -> Result<Self, &'static str> {
        if pinned_revisions.iter().all(|pin| pin.trim().is_empty()) {
            return Err(TWIN_PIN_MISSING);
        }
        Ok(Self {
            agent_ref: agent_ref.into(),
            pinned_revisions,
        })
    }

    pub fn agent_ref(&self) -> &str {
        &self.agent_ref
    }

    pub fn pinned_revisions(&self) -> &[String] {
        &self.pinned_revisions
    }
}

impl TryFrom<AgentTwinWire> for AgentTwin {
    type Error = &'static str;

    fn try_from(wire: AgentTwinWire) -> Result<Self, Self::Error> {
        Self::new(wire.agent_ref, wire.pinned_revisions)
    }
}

/// The closed lifecycle of an agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Registered,
    Active,
    Draining,
    Terminated,
}

/// A running instance of an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInstance {
    pub instance_id: String,
    pub definition_ref: String,
    pub state: AgentLifecycleState,
    pub mailbox_ref: String,
    pub activation_sources: Vec<String>,
}

/// The open work of one team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamTodo {
    pub team_id: String,
    pub items: Vec<OrgPlanItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
    }

    // spec: MEDULLA-ORCH-VOCAB-R750.750
    #[test]
    fn records_round_trip_and_twin_needs_a_pin() {
        let item = OrgPlanItem {
            item_id: "i2".into(),
            role_id: "reviewer".into(),
            depends_on: vec!["i1".into()],
        };
        round_trip(&OrgChart {
            org_id: "org:a".into(),
            roles: vec![RoleSpec {
                role_id: "reviewer".into(),
                required_capabilities: vec!["cap:review".into()],
                headcount: 2,
            }],
        });
        round_trip(&EmployeeSpec {
            agent_ref: "agent:r".into(),
            definition_revision: "rev:3".into(),
            role_id: "reviewer".into(),
        });
        round_trip(&item);
        round_trip(&TeamTodo {
            team_id: "team:a".into(),
            items: vec![item],
        });
        for state in [
            AgentLifecycleState::Registered,
            AgentLifecycleState::Active,
            AgentLifecycleState::Draining,
            AgentLifecycleState::Terminated,
        ] {
            round_trip(&AgentInstance {
                instance_id: "inst:1".into(),
                definition_ref: "agent:r".into(),
                state,
                mailbox_ref: "mbox:1".into(),
                activation_sources: vec!["schedule".into()],
            });
        }
        assert!(serde_json::from_str::<AgentLifecycleState>("\"paused\"").is_err());

        let twin = AgentTwin::new("agent:r", vec!["rev:3".into()]).unwrap();
        round_trip(&twin);
        assert_eq!(twin.pinned_revisions(), ["rev:3".to_string()]);
        assert_eq!(AgentTwin::new("agent:r", vec![]), Err(TWIN_PIN_MISSING));
        let unpinned = r#"{"agent_ref":"agent:r","pinned_revisions":[]}"#;
        assert!(serde_json::from_str::<AgentTwin>(unpinned).is_err());
    }
}
