//! The closed registry of work-item kinds the engine admits
//! (MEDULLA-UNIFIED-DATA-PLANE-R766).

use serde::{Deserialize, Serialize};

/// The closed registry of work-item kinds the engine admits
/// (MEDULLA-UNIFIED-DATA-PLANE-R766). Every wire string is the one submitted
/// before the registry existed; a rename would be a wire change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkItemKind {
    /// A browser-control session step.
    #[serde(rename = "browser.control")]
    BrowserControl,
    /// A failure-cluster analysis.
    #[serde(rename = "failure_cluster")]
    FailureCluster,
    /// A remediation of a recorded gap.
    #[serde(rename = "gap_remediation")]
    GapRemediation,
    /// An authenticated replay test.
    #[serde(rename = "authenticated-replay-test")]
    AuthenticatedReplayTest,
    /// Web research submitted by the `research.web` operation.
    #[serde(rename = "research_web")]
    ResearchWeb,
    /// A host start.
    #[serde(rename = "host.start")]
    HostStart,
    /// A delegated agent run (`DelegationRequest.kind`).
    #[serde(rename = "agent.execute")]
    AgentExecute,
    /// One agent turn on the durable agent-turn queue (`agent_turn`).
    #[serde(rename = "agent_turn")]
    AgentTurn,
}

impl WorkItemKind {
    /// Every admitted kind, in registry order.
    pub const REGISTRY: [WorkItemKind; 8] = [
        Self::BrowserControl,
        Self::FailureCluster,
        Self::GapRemediation,
        Self::AuthenticatedReplayTest,
        Self::ResearchWeb,
        Self::HostStart,
        Self::AgentExecute,
        Self::AgentTurn,
    ];

    /// The wire string of this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BrowserControl => "browser.control",
            Self::FailureCluster => "failure_cluster",
            Self::GapRemediation => "gap_remediation",
            Self::AuthenticatedReplayTest => "authenticated-replay-test",
            Self::ResearchWeb => crate::host_start::RESEARCH_WEB_KIND,
            Self::HostStart => crate::host_start::HOST_START_KIND,
            Self::AgentExecute => "agent.execute",
            Self::AgentTurn => "agent_turn",
        }
    }

    /// Look a wire string up in the registry; an unknown kind is `None`.
    pub fn from_wire(kind: &str) -> Option<Self> {
        Self::REGISTRY
            .into_iter()
            .find(|candidate| candidate.as_str() == kind)
    }
}
