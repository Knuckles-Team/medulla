//! MEDULLA-DECISION-ENGINE-R100.101.1: the typed inputs and result of grounded
//! topic coverage. The kernel lives in `medulla-decision-stats`
//! (`decision::grounded_eval::topic_coverage`).

use serde::{Deserialize, Serialize};

use crate::decision::numeric::UnitRationalWire;

/// The reach of the corpus a grounded answer may draw on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GroundedScope {
    SingleDocument,
    LocalMultiDocument,
    GlobalCorpus,
}

/// The topics one query's answer is expected to cover: non-empty, unique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExpectedTopicSet {
    pub query_id: String,
    pub topic_ids: Vec<String>,
}

/// Valid independent support votes for one citation: `supporting` of `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CitationSupport {
    pub citation_id: String,
    pub supporting: u32,
    pub total: u32,
}

/// One answer claim and the citations it binds to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimSupport {
    pub claim_id: String,
    pub citations: Vec<CitationSupport>,
}

/// One topic of one query bound to its answer claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TopicSupport {
    pub query_id: String,
    pub topic_id: String,
    pub claims: Vec<ClaimSupport>,
}

/// `knob grounded.min_topic_support` (default 1/2, [1/2, 1], ratio) and
/// `knob grounded.min_valid_votes` (default 3, [1, 25], votes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedKnobs {
    pub min_topic_support: UnitRationalWire,
    pub min_valid_votes: u32,
}

impl Default for GroundedKnobs {
    fn default() -> Self {
        Self {
            min_topic_support: UnitRationalWire::new(1, 2).expect("one half is a unit rational"),
            min_valid_votes: 3,
        }
    }
}

/// The expected and covered topics of one query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryTopicCoverage {
    pub query_id: String,
    pub expected_topic_ids: Vec<String>,
    pub covered_topic_ids: Vec<String>,
}

/// Pooled coverage of one scope: summed covered over summed expected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScopeTopicCoverage {
    pub scope: GroundedScope,
    pub queries: Vec<QueryTopicCoverage>,
    pub covered: u64,
    pub expected: u64,
}
