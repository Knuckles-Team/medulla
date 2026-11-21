//! The `recruit` question (MEDULLA-ORCH-VOCAB-R750.751).
//!
//! `Decide` with
//! [`QuestionKind::Recruit`](super::statistical::QuestionKind::Recruit) ranks
//! the employees offered for one role by capability coverage, allocates the
//! top legal market offer, and replays a twin against its pinned revisions.
//! The records are the organization and market records of
//! `medulla_work_types::orchestration::org`; this module adds only the
//! request and answer that carry them. Nothing here reads a clock or a store,
//! and `Decide` writes nothing for this kind.

use serde::{Deserialize, Serialize};

use medulla_work_types::orchestration::org::{
    AgentTwin, EmployeeSpec, MarketFacts, OfferSelection, RoleSpec,
};
use medulla_work_types::work_market::GapView;

/// One employee offered for the role and the capabilities it declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecruitCandidate {
    pub employee: EmployeeSpec,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The market the allocation selects from: the gap views and the resource
/// facts `derive_legal_offers` runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecruitMarket {
    pub gaps: Vec<GapView>,
    pub facts: MarketFacts,
}

/// The request of a `recruit` question, carried as the JSON text parameter
/// [`PARAM_RECRUIT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecruitRequest {
    pub role: RoleSpec,
    pub candidates: Vec<RecruitCandidate>,
    /// The market to allocate from; `None` asks for the ranking only.
    #[serde(default)]
    pub market: Option<RecruitMarket>,
    /// A twin to replay against: an employee of the twin's agent is
    /// considered only at one of the twin's pinned revisions.
    #[serde(default)]
    pub twin: Option<AgentTwin>,
}

/// One ranked employee and how much of the role it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedEmployee {
    pub agent_ref: String,
    pub definition_revision: String,
    /// Required capabilities of the role the employee declares.
    pub covered: u32,
    /// Distinct required capabilities of the role.
    pub required: u32,
}

/// The answer of a `recruit` question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecruitResult {
    pub role_id: String,
    /// Best coverage first; ties by the lowest agent IRI, then revision.
    pub ranked: Vec<RankedEmployee>,
    /// The allocation: at most the top legal offer, and every excluded Gap
    /// with its `OfferExclusion` code. `None` when no market was given.
    #[serde(default)]
    pub allocation: Option<OfferSelection>,
}

/// The request parameter carrying the [`RecruitRequest`].
pub const PARAM_RECRUIT: &str = "recruit";
/// The one option a `recruit` question declares.
pub const RECRUIT_OPTION: &str = "recruit";
/// The prefix of the answer option that carries the [`RecruitResult`] as JSON.
pub const ANSWER_PREFIX: &str = "recruit:";
