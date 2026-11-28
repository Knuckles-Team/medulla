//! The standalone topology question (MEDULLA-DECISION-ENGINE-R111.1).
//!
//! `Decide` of kind `Topology` asks for the swarm shape of ONE published graph
//! template: how wide each slot is, how many rounds it runs, and what capacity
//! the plan would lease. The caller supplies the typed [`TopologyQuestion`] as
//! the single parameter named [`TOPOLOGY_PARAM`]; the engine fills
//! `TopologyQuestion::read` (schema entailments and capacity headroom) from its
//! own stores before deciding, so the recorded inputs carry the facts a replay
//! reads. A caller can never supply `read` itself: observed headroom is not a
//! caller claim.

use medulla_agent_types::agent_topology::{TopologyInputs, TopologyRequirements};
use medulla_agent_types::delegation::AgentGraphEntryRef;
use serde::{Deserialize, Serialize};

/// The name of the one parameter a topology question carries.
pub const TOPOLOGY_PARAM: &str = "topology";

/// The refusal code when the request graph has no usable topology schema.
pub const TOPOLOGY_SCHEMA_UNAVAILABLE: &str = "TOPOLOGY_SCHEMA_UNAVAILABLE";

/// Which template is asked, in which graph, under which requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TopologyQuestion {
    /// The graph whose composed schema entails admissibility and whose
    /// capacity cells give the headroom; read under the caller's access.
    pub graph: String,
    /// The published template the question is asked of. Choosing among
    /// templates is `AgentAssemble`'s job, not this question's.
    pub template: AgentGraphEntryRef,
    /// Task classes, demand, deadline, caps and capacity scope.
    pub requirements: TopologyRequirements,
    /// Engine-filled: the schema entailments and capacity headroom read for
    /// this question. A caller-supplied value is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<TopologyInputs>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_fixtures::topology::requirements;

    fn question() -> TopologyQuestion {
        TopologyQuestion {
            graph: "g".into(),
            template: AgentGraphEntryRef {
                tenant_id: "t".into(),
                graph_id: "tpl".into(),
                entry_revision: 1,
                definition_digest: "a".repeat(64),
                actor_scope: "operator".into(),
                purpose_id: "agent-graph:publish".into(),
                policy_digest: "b".repeat(64),
                composed_work_ceiling: 8,
            },
            requirements: requirements(1),
            read: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R111.1
    #[test]
    fn question_round_trips_and_omits_unread_facts() {
        let asked = question();
        let json = serde_json::to_string(&asked).unwrap();
        assert!(!json.contains("\"read\""), "{json}");
        let back: TopologyQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(back, asked);
    }
}
