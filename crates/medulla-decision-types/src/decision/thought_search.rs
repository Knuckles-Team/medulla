//! Bounded multi-step reasoning search: the wire model (MEDULLA-DECISION-ENGINE-R750.750).
//!
//! The caller keeps the thought arena and sends a bounded frontier summary,
//! [`ReasoningState`], with its spent budget. `Decide` answers with a
//! [`ThoughtSearchAnswer`]: which leaves to expand next, or a stop with the
//! best-so-far node. Scores are caller claims on the `Q32` scale. Nothing here
//! reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::{QuantScaleTag, QuantisedValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;
use super::statistical::StatisticalErrorCode;
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

    /// The structural check the served family runs before it chooses
    /// (MEDULLA-DECISION-ENGINE-R750.752). Arena order puts a parent before
    /// its children, so a parent id must be smaller than the child's. With
    /// `merge` (the `Graph` strategy), two thoughts that reached the same
    /// state as an ancestor and its descendant would merge into one thought
    /// that is its own descendant.
    pub fn validate(&self, merge: bool) -> Result<(), ThoughtFault> {
        let nodes = self.nodes.as_slice();
        let mut by_id: std::collections::BTreeMap<u32, &ThoughtNode> =
            std::collections::BTreeMap::new();
        for node in nodes {
            if node.score.scale != QuantScaleTag::Q32 {
                return Err(ThoughtFault::ScoreNotFixedPoint { id: node.id });
            }
            if by_id.insert(node.id, node).is_some() {
                return Err(ThoughtFault::NodeInvalid {
                    id: node.id,
                    detail: "duplicate id",
                });
            }
        }
        for node in nodes {
            let invalid = |detail| ThoughtFault::NodeInvalid {
                id: node.id,
                detail,
            };
            match node.parent {
                None if node.depth != 0 => return Err(invalid("a root has depth 0")),
                None => {}
                Some(parent) if parent >= node.id => {
                    return Err(invalid("the parent is not an earlier arena index"))
                }
                Some(_) if node.depth == 0 => return Err(invalid("a child has depth above 0")),
                Some(parent) => {
                    if let Some(parent) = by_id.get(&parent) {
                        if node.depth != parent.depth.saturating_add(1) {
                            return Err(invalid("the depth disagrees with the parent"));
                        }
                    }
                }
            }
            if merge {
                let mut cursor = node.parent;
                while let Some(id) = cursor {
                    let Some(ancestor) = by_id.get(&id) else {
                        break;
                    };
                    if ancestor.state_digest == node.state_digest {
                        return Err(ThoughtFault::MergeCycle { id: node.id });
                    }
                    cursor = ancestor.parent;
                }
            }
        }
        Ok(())
    }
}

/// Why a reasoning state is refused (MEDULLA-DECISION-ENGINE-R750.752).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThoughtFault {
    /// A duplicate id, a missing parent or a depth that disagrees with it.
    NodeInvalid { id: u32, detail: &'static str },
    /// A score outside scale Q32.
    ScoreNotFixedPoint { id: u32 },
    /// A graph merge that makes a thought its own descendant.
    MergeCycle { id: u32 },
}

impl ThoughtFault {
    /// The stable refusal text, `"CODE: detail"`.
    pub fn refusal(self) -> String {
        match self {
            Self::NodeInvalid { id, detail } => {
                StatisticalErrorCode::ThoughtNodeInvalid.refusal(format!("thought {id}: {detail}"))
            }
            Self::ScoreNotFixedPoint { id } => StatisticalErrorCode::ThoughtScoreNotFixedPoint
                .refusal(format!("thought {id}: the score is not on scale Q32")),
            Self::MergeCycle { id } => StatisticalErrorCode::ThoughtMergeCycle.refusal(format!(
                "thought {id}: the merge makes it its own descendant"
            )),
        }
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

/// The refusal codes the `thought_search` family declares
/// (MEDULLA-DECISION-ENGINE-R750.752); each is also a `StatisticalErrorCode`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("THOUGHT_STRATEGY_UNKNOWN"),
    RefusalCode::new("THOUGHT_BUDGET_INVALID"),
    RefusalCode::new("THOUGHT_MERGE_CYCLE"),
    RefusalCode::new("THOUGHT_SCORE_NOT_FIXED_POINT"),
    RefusalCode::new("THOUGHT_NODE_INVALID"),
];

const fn spec(
    name: &'static str,
    unit: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default: KnobValue::Integer(default),
        min: KnobValue::Integer(min),
        max: KnobValue::Integer(max),
        safety: KnobSafety::Tunable,
    }
}

/// The `thought_search.*` decision knobs (MEDULLA-DECISION-ENGINE-R750.750),
/// in the field order of [`ThoughtSearchKnobs`].
pub const KNOBS: &[KnobSpec] = &[
    spec("thought_search.max_nodes", "nodes", 100_000, 1, 1_000_000),
    spec("thought_search.max_depth", "levels", 16, 1, 64),
    spec("thought_search.max_branching", "children", 4, 1, 16),
    spec(
        "thought_search.max_tokens",
        "tokens",
        200_000,
        1_000,
        4_000_000,
    ),
    spec("thought_search.beam_width", "nodes", 3, 1, 16),
];

impl ThoughtSearchKnobs {
    /// The values the pinned policy sets, each knob or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
            _ => 0,
        };
        let narrow = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
        Self {
            max_nodes: read(0),
            max_depth: narrow(read(1)),
            max_branching: narrow(read(2)),
            max_tokens: read(3),
            beam_width: narrow(read(4)),
        }
    }

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
        assert_eq!(
            ThoughtSearchKnobs::from_policy(&DecisionPolicy::engine_default()),
            ThoughtSearchKnobs::default()
        );
        assert!(ThoughtSearchKnobs {
            beam_width: 17,
            ..ThoughtSearchKnobs::default()
        }
        .checked()
        .is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R750.752
    #[test]
    fn declared_refusals_are_registered_statistical_codes() {
        for code in REFUSALS {
            assert_eq!(super::super::codes::resolve(code.as_str()), Some(*code));
            assert!(StatisticalErrorCode::ALL
                .iter()
                .any(|known| known.as_str() == code.as_str()));
        }
    }

    #[test]
    fn validation_refuses_each_fault() {
        let ok = ReasoningState::new(vec![node(0), node(1)], BudgetLedger::default()).unwrap();
        assert!(ok.validate(true).is_ok());
        let check = |nodes: Vec<ThoughtNode>, merge: bool| {
            ReasoningState::new(nodes, BudgetLedger::default())
                .unwrap()
                .validate(merge)
                .unwrap_err()
                .refusal()
        };
        assert!(check(vec![node(1), node(1)], false).starts_with("THOUGHT_NODE_INVALID"));
        let mut orphan = node(1);
        orphan.parent = Some(5);
        assert!(check(vec![orphan], false).starts_with("THOUGHT_NODE_INVALID"));
        let mut deep = node(1);
        deep.depth = 4;
        assert!(check(vec![node(0), deep], false).starts_with("THOUGHT_NODE_INVALID"));
        let mut float = node(0);
        float.score.scale = QuantScaleTag::Pico;
        assert!(check(vec![float], false).starts_with("THOUGHT_SCORE_NOT_FIXED_POINT"));
        let mut twin = node(1);
        twin.state_digest = node(0).state_digest;
        assert!(check(vec![node(0), twin.clone()], true).starts_with("THOUGHT_MERGE_CYCLE"));
        assert!(
            ReasoningState::new(vec![node(0), twin], BudgetLedger::default())
                .unwrap()
                .validate(false)
                .is_ok()
        );
    }
}
