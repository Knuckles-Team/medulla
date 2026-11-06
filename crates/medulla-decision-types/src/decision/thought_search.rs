//! Bounded multi-step reasoning search: the wire model (MEDULLA-DECISION-ENGINE-R750.750).
//!
//! The caller keeps the thought arena and sends a bounded frontier summary,
//! [`ReasoningState`], with its spent budget. `Decide` answers with a
//! [`ThoughtSearchAnswer`]: which leaves to expand next, or a stop with the
//! best-so-far node. Scores are caller claims on the `Q32` scale. Nothing here
//! reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::numeric::QuantisedValue;
use medulla_types::contract::BoundedVec;

/// Most expandable leaves one reasoning state may carry.
pub const MAX_THOUGHT_LEAVES: usize = 64;

/// How the frontier is expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReasoningStrategy {
    /// One leaf at a time, the best one.
    Chain,
    /// A beam over the shallowest leaves.
    Tree,
    /// Like `Tree`, after merging leaves that reached the same state.
    Graph,
    /// A beam over every depth.
    Planning,
}

/// One thought in the caller's arena.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ThoughtNode {
    /// Arena index.
    pub id: u32,
    /// Arena index of the parent; `None` for the root.
    #[serde(default)]
    pub parent: Option<u32>,
    pub depth: u32,
    /// The caller's score on `Q32`.
    pub score: QuantisedValue,
    pub tokens: u32,
    /// Digest of the reasoning state this thought reached.
    pub state_digest: String,
}

/// What the caller has spent so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BudgetLedger {
    pub nodes_used: u64,
    pub tokens_used: u64,
    pub depth_reached: u32,
}

/// The bounded frontier summary: the expandable leaves and the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReasoningState {
    pub nodes: BoundedVec<ThoughtNode, MAX_THOUGHT_LEAVES>,
    pub ledger: BudgetLedger,
}

impl ReasoningState {
    /// The validating constructor: at most [`MAX_THOUGHT_LEAVES`] leaves.
    pub fn new(nodes: Vec<ThoughtNode>, ledger: BudgetLedger) -> Result<Self, String> {
        Ok(Self {
            nodes: BoundedVec::new(nodes)?,
            ledger,
        })
    }
}

/// The answer of one thought-search decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ThoughtSearchAnswer {
    /// Expand these leaves next, in order.
    Expand {
        node_ids: BoundedVec<u32, MAX_THOUGHT_LEAVES>,
    },
    /// Stop with the best-so-far node.
    Stop {
        best_node: u32,
        /// `sha256:<hex>` of the root-to-best node ids and scores.
        path_digest: String,
        /// Set when a budget, not the depth limit, stopped the search.
        certified_gap: bool,
        /// Highest unexpanded score minus the best score, floored at 0, on `Q32`.
        gap: QuantisedValue,
    },
}

/// The pinned `knob thought_search.*` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ThoughtSearchKnobs {
    /// `knob thought_search.max_nodes`, range [1, 1000000], unit nodes.
    pub max_nodes: u64,
    /// `knob thought_search.max_depth`, range [1, 64], unit levels.
    pub max_depth: u32,
    /// `knob thought_search.max_branching`, range [1, 16], unit children.
    pub max_branching: u32,
    /// `knob thought_search.max_tokens`, range [1000, 4000000], unit tokens.
    pub max_tokens: u64,
    /// `knob thought_search.beam_width`, range [1, 16], unit nodes.
    pub beam_width: u32,
}

impl Default for ThoughtSearchKnobs {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_depth: 16,
            max_branching: 4,
            max_tokens: 200_000,
            beam_width: 3,
        }
    }
}

impl ThoughtSearchKnobs {
    /// The validating constructor: every knob inside its declared range.
    pub fn checked(self) -> Result<Self, String> {
        let in_range = (1..=1_000_000).contains(&self.max_nodes)
            && (1..=64).contains(&self.max_depth)
            && (1..=16).contains(&self.max_branching)
            && (1_000..=4_000_000).contains(&self.max_tokens)
            && (1..=16).contains(&self.beam_width);
        in_range
            .then_some(self)
            .ok_or_else(|| "thought_search knob outside its range".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::numeric::QuantScaleTag;
    use super::*;

    fn node(id: u32) -> ThoughtNode {
        ThoughtNode {
            id,
            parent: id.checked_sub(1),
            depth: id,
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: i64::from(id) << 20,
            },
            tokens: 10,
            state_digest: format!("sha256:{id:064x}"),
        }
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let text = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), v);
    }

    #[test]
    fn thought_types_round_trip() {
        for strategy in [
            ReasoningStrategy::Chain,
            ReasoningStrategy::Tree,
            ReasoningStrategy::Graph,
            ReasoningStrategy::Planning,
        ] {
            round_trip(&strategy);
        }
        assert_eq!(
            serde_json::to_string(&ReasoningStrategy::Planning).unwrap(),
            "\"planning\""
        );
        let ledger = BudgetLedger {
            nodes_used: 3,
            tokens_used: 30,
            depth_reached: 2,
        };
        round_trip(&ledger);
        round_trip(&node(0));
        let state = ReasoningState::new((0..64).map(node).collect(), ledger).unwrap();
        round_trip(&state);
        round_trip(&ThoughtSearchAnswer::Expand {
            node_ids: BoundedVec::new(vec![1, 2]).unwrap(),
        });
        round_trip(&ThoughtSearchAnswer::Stop {
            best_node: 2,
            path_digest: "sha256:00".into(),
            certified_gap: true,
            gap: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 0,
            },
        });
        let knobs = ThoughtSearchKnobs::default().checked().unwrap();
        round_trip(&knobs);
        assert_eq!(
            serde_json::to_string(&crate::decision::QuestionKind::ThoughtSearch).unwrap(),
            "\"thought_search\""
        );

        // A 65th node is refused by the constructor and by the decoder.
        assert!(ReasoningState::new((0..65).map(node).collect(), ledger).is_err());
        let mut wire = serde_json::to_value(&state).unwrap();
        wire["nodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(node(64)).unwrap());
        assert!(serde_json::from_value::<ReasoningState>(wire).is_err());
        assert!(ThoughtSearchKnobs {
            beam_width: 17,
            ..ThoughtSearchKnobs::default()
        }
        .checked()
        .is_err());
    }
}
