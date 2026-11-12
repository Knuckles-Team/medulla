//! What admitting a topology plan against the sandbox caps records, and why it
//! refuses (MEDULLA-DECISION-ENGINE-R046.750).
//!
//! The plan never grants itself more than the caps: `admit_topology` in the
//! compute crate compares every slot's width, rounds and tokens, and the
//! plan's depth, with the caps, and answers an [`EnforcementRecord`] naming the
//! caps it held the plan to, or the [`TopologyRefusal`] of the first breach.

use serde::{Deserialize, Serialize};

use super::TopologyCaps;
use super::TopologyPlan;
use medulla_types::refusal_code::RefusalCode;

/// A slot width above the width cap.
pub const TOPOLOGY_WIDTH_EXCEEDS_CAP: RefusalCode = RefusalCode::new("TOPOLOGY_WIDTH_EXCEEDS_CAP");
/// A plan nested deeper than the depth cap.
pub const TOPOLOGY_DEPTH_EXCEEDS_CAP: RefusalCode = RefusalCode::new("TOPOLOGY_DEPTH_EXCEEDS_CAP");
/// A slot running more rounds than the rounds cap.
pub const TOPOLOGY_ROUNDS_EXCEEDS_CAP: RefusalCode =
    RefusalCode::new("TOPOLOGY_ROUNDS_EXCEEDS_CAP");
/// A plan declaring more tokens than the tokens cap.
pub const TOPOLOGY_TOKENS_EXCEEDS_CAP: RefusalCode =
    RefusalCode::new("TOPOLOGY_TOKENS_EXCEEDS_CAP");

/// Every admission refusal code, in declaration order.
pub const REFUSALS: &[RefusalCode] = &[
    TOPOLOGY_WIDTH_EXCEEDS_CAP,
    TOPOLOGY_DEPTH_EXCEEDS_CAP,
    TOPOLOGY_ROUNDS_EXCEEDS_CAP,
    TOPOLOGY_TOKENS_EXCEEDS_CAP,
];

/// Hash domain of a plan inside its enforcement record.
pub const TOPOLOGY_PLAN_DIGEST_DOMAIN: &str = "medulla/topology-plan/v1";

/// The caps an admitted plan was held to, and the plan it was held for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EnforcementRecord {
    /// `sha256:<hex>` of the admitted plan.
    pub plan_digest: String,
    pub width_cap: u8,
    pub depth_cap: u8,
    pub rounds_cap: u8,
    /// The ceiling on the plan's declared tokens, when the caps name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_cap: Option<u64>,
}

impl EnforcementRecord {
    /// The record of holding `plan` to `caps`.
    pub fn of(plan: &TopologyPlan, caps: &TopologyCaps) -> Self {
        Self {
            plan_digest: plan_digest(plan),
            width_cap: caps.max_width,
            depth_cap: caps.max_depth,
            rounds_cap: caps.max_rounds,
            tokens_cap: caps.max_tokens,
        }
    }

    /// The `(name, value)` of every cap, in a fixed order: the premise facts
    /// are `topology_cap:<name>=<value>`.
    pub fn cap_facts(&self) -> Vec<(&'static str, String)> {
        let mut facts = vec![
            ("max_width", self.width_cap.to_string()),
            ("max_depth", self.depth_cap.to_string()),
            ("max_rounds", self.rounds_cap.to_string()),
        ];
        if let Some(tokens) = self.tokens_cap {
            facts.push(("max_tokens", tokens.to_string()));
        }
        facts
    }
}

/// `sha256:<hex>` of `plan` in the decision surface's one digest scheme.
pub fn plan_digest(plan: &TopologyPlan) -> String {
    let digest = String::from(medulla_query_types::solve_scalar::Sha256Digest::of_json(
        TOPOLOGY_PLAN_DIGEST_DOMAIN,
        plan,
    ));
    format!("sha256:{digest}")
}

/// The first breach an admission found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TopologyRefusal {
    WidthExceedsCap {
        node_id: String,
        width: u8,
        cap: u8,
    },
    DepthExceedsCap {
        depth: u8,
        cap: u8,
    },
    RoundsExceedsCap {
        node_id: String,
        rounds: u8,
        cap: u8,
    },
    TokensExceedsCap {
        tokens: u64,
        cap: u64,
    },
}

impl TopologyRefusal {
    /// The registered wire code.
    pub const fn code(&self) -> RefusalCode {
        match self {
            Self::WidthExceedsCap { .. } => TOPOLOGY_WIDTH_EXCEEDS_CAP,
            Self::DepthExceedsCap { .. } => TOPOLOGY_DEPTH_EXCEEDS_CAP,
            Self::RoundsExceedsCap { .. } => TOPOLOGY_ROUNDS_EXCEEDS_CAP,
            Self::TokensExceedsCap { .. } => TOPOLOGY_TOKENS_EXCEEDS_CAP,
        }
    }
}

impl std::fmt::Display for TopologyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code().as_str();
        match self {
            Self::WidthExceedsCap {
                node_id,
                width,
                cap,
            } => write!(f, "{code}: slot {node_id} width {width} over cap {cap}"),
            Self::DepthExceedsCap { depth, cap } => {
                write!(f, "{code}: depth {depth} over cap {cap}")
            }
            Self::RoundsExceedsCap {
                node_id,
                rounds,
                cap,
            } => write!(f, "{code}: slot {node_id} rounds {rounds} over cap {cap}"),
            Self::TokensExceedsCap { tokens, cap } => {
                write!(f, "{code}: tokens {tokens} over cap {cap}")
            }
        }
    }
}

impl std::error::Error for TopologyRefusal {}
