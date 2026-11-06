//! Thought-search expansion choice (MEDULLA-DECISION-ENGINE-R750.751).
//!
//! [`choose_expansion`] is a pure, deterministic function of the caller's
//! frontier summary, the pinned strategy and the pinned knobs: no clock, no
//! store, no randomness. It either names the leaves to expand next or stops
//! with the best-so-far node and its path digest.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use medulla_decision_types::decision::digest::thought_path_digest;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::thought_search::{
    ReasoningState, ReasoningStrategy, ThoughtNode, ThoughtSearchAnswer, ThoughtSearchKnobs,
};
use medulla_types::contract::BoundedVec;

/// Score descending, then id ascending.
fn rank_key(node: &ThoughtNode) -> (Reverse<i64>, u32) {
    (Reverse(node.score.value), node.id)
}

/// `Graph` merge: leaves with equal `state_digest` collapse into the lowest
/// id, which keeps the highest score of the group.
fn merge_equal_states(nodes: &[ThoughtNode]) -> Vec<ThoughtNode> {
    let mut groups: BTreeMap<&str, ThoughtNode> = BTreeMap::new();
    for node in nodes {
        groups
            .entry(node.state_digest.as_str())
            .and_modify(|kept| {
                let score = kept.score.value.max(node.score.value);
                if node.id < kept.id {
                    *kept = node.clone();
                }
                kept.score.value = score;
            })
            .or_insert_with(|| node.clone());
    }
    groups.into_values().collect()
}

/// The best node: highest score, ties by id ascending.
fn best_of(nodes: &[ThoughtNode]) -> Option<&ThoughtNode> {
    nodes.iter().min_by_key(|n| rank_key(n))
}

/// The root-to-best path over the nodes present in the summary, following
/// `parent` links until a parent is absent.
fn path_to(nodes: &[ThoughtNode], best: &ThoughtNode) -> Vec<(u32, QuantisedValue)> {
    let by_id: BTreeMap<u32, &ThoughtNode> = nodes.iter().map(|n| (n.id, n)).collect();
    let mut path = vec![(best.id, best.score)];
    let mut cursor = best.parent;
    while let Some(id) = cursor {
        let Some(node) = by_id.get(&id) else { break };
        if path.iter().any(|(seen, _)| *seen == id) {
            break;
        }
        path.push((node.id, node.score));
        cursor = node.parent;
    }
    path.reverse();
    path
}

fn stop(nodes: &[ThoughtNode], expandable: &[ThoughtNode], budget: bool) -> ThoughtSearchAnswer {
    let Some(best) = best_of(nodes) else {
        return ThoughtSearchAnswer::Stop {
            best_node: 0,
            path_digest: thought_path_digest(&[]),
            certified_gap: budget,
            gap: q32(0),
        };
    };
    let highest_unexpanded = expandable
        .iter()
        .map(|n| n.score.value)
        .max()
        .unwrap_or(best.score.value);
    ThoughtSearchAnswer::Stop {
        best_node: best.id,
        path_digest: thought_path_digest(&path_to(nodes, best)),
        certified_gap: budget,
        gap: q32(highest_unexpanded.saturating_sub(best.score.value).max(0)),
    }
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

/// Choose which leaves to expand next, or stop.
///
/// Stops when `nodes_used` reaches `max_nodes`, `tokens_used` reaches
/// `max_tokens` (both budget stops, `certified_gap` set), or no leaf is below
/// `max_depth`. Otherwise expands `n` leaves: the strategy width limited by
/// the remaining node budget divided by `max_branching`; `n == 0` is a budget
/// stop.
pub fn choose_expansion(
    state: &ReasoningState,
    strategy: ReasoningStrategy,
    knobs: &ThoughtSearchKnobs,
) -> ThoughtSearchAnswer {
    let nodes: Vec<ThoughtNode> = match strategy {
        ReasoningStrategy::Graph => merge_equal_states(state.nodes.as_slice()),
        _ => state.nodes.as_slice().to_vec(),
    };
    let expandable: Vec<ThoughtNode> = nodes
        .iter()
        .filter(|n| n.depth < knobs.max_depth)
        .cloned()
        .collect();
    let ledger = &state.ledger;
    if ledger.nodes_used >= knobs.max_nodes || ledger.tokens_used >= knobs.max_tokens {
        return stop(&nodes, &expandable, true);
    }
    if expandable.is_empty() {
        return stop(&nodes, &expandable, false);
    }
    let width = match strategy {
        ReasoningStrategy::Chain => 1,
        _ => u64::from(knobs.beam_width),
    };
    let by_budget = (knobs.max_nodes - ledger.nodes_used) / u64::from(knobs.max_branching.max(1));
    let n = usize::try_from(width.min(by_budget)).unwrap_or(usize::MAX);
    if n == 0 {
        return stop(&nodes, &expandable, true);
    }
    let mut pool: Vec<&ThoughtNode> = match strategy {
        ReasoningStrategy::Tree | ReasoningStrategy::Graph => {
            let least = expandable.iter().map(|n| n.depth).min().unwrap_or(0);
            expandable.iter().filter(|n| n.depth == least).collect()
        }
        ReasoningStrategy::Chain | ReasoningStrategy::Planning => expandable.iter().collect(),
    };
    pool.sort_by_key(|n| rank_key(n));
    let ids: Vec<u32> = pool.into_iter().take(n).map(|n| n.id).collect();
    ThoughtSearchAnswer::Expand {
        node_ids: BoundedVec::new(ids).expect("at most the leaves of a bounded state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::thought_search::BudgetLedger;

    fn node(id: u32, parent: Option<u32>, depth: u32, score: i64, state: &str) -> ThoughtNode {
        ThoughtNode {
            id,
            parent,
            depth,
            score: q32(score),
            tokens: 10,
            state_digest: state.to_string(),
        }
    }

    fn state(nodes: Vec<ThoughtNode>, nodes_used: u64, tokens_used: u64) -> ReasoningState {
        ReasoningState::new(
            nodes,
            BudgetLedger {
                nodes_used,
                tokens_used,
                depth_reached: 2,
            },
        )
        .unwrap()
    }

    fn frontier() -> Vec<ThoughtNode> {
        vec![
            node(1, Some(0), 1, 50, "a"),
            node(2, Some(0), 1, 70, "b"),
            node(3, Some(1), 2, 90, "c"),
            node(4, Some(1), 2, 70, "b"),
            node(5, Some(0), 1, 70, "d"),
        ]
    }

    fn expanded(answer: ThoughtSearchAnswer) -> Vec<u32> {
        match answer {
            ThoughtSearchAnswer::Expand { node_ids } => node_ids.as_slice().to_vec(),
            other => panic!("expected expansion, got {other:?}"),
        }
    }

    #[test]
    fn strategies_pick_the_documented_leaves() {
        let knobs = ThoughtSearchKnobs {
            beam_width: 2,
            ..ThoughtSearchKnobs::default()
        };
        let s = state(frontier(), 5, 50);
        // Chain: the single highest-scored leaf.
        assert_eq!(
            expanded(choose_expansion(&s, ReasoningStrategy::Chain, &knobs)),
            vec![3]
        );
        // Tree: least depth (1), score descending, ties by id ascending.
        assert_eq!(
            expanded(choose_expansion(&s, ReasoningStrategy::Tree, &knobs)),
            vec![2, 5]
        );
        // Planning: every depth, score descending.
        assert_eq!(
            expanded(choose_expansion(&s, ReasoningStrategy::Planning, &knobs)),
            vec![3, 2]
        );
        // Graph: 2 and 4 share state "b" and merge into 2; then Tree.
        let mut graph = frontier();
        graph[1].score = q32(40);
        graph[3].score = q32(80);
        let g = state(graph, 5, 50);
        assert_eq!(
            expanded(choose_expansion(&g, ReasoningStrategy::Graph, &knobs)),
            vec![2, 5]
        );
        // Depth limit: no leaf below max_depth stops without a budget gap.
        let shallow = ThoughtSearchKnobs {
            max_depth: 1,
            ..knobs
        };
        match choose_expansion(&s, ReasoningStrategy::Tree, &shallow) {
            ThoughtSearchAnswer::Stop {
                best_node,
                certified_gap,
                ..
            } => {
                assert_eq!(best_node, 3);
                assert!(!certified_gap);
            }
            other => panic!("expected stop, got {other:?}"),
        }
        // The node budget limits the width: one node left, branching 4.
        let tight = ThoughtSearchKnobs {
            max_nodes: 9,
            max_branching: 4,
            beam_width: 3,
            ..ThoughtSearchKnobs::default()
        };
        assert_eq!(
            expanded(choose_expansion(
                &state(frontier(), 5, 50),
                ReasoningStrategy::Planning,
                &tight
            )),
            vec![3]
        );
    }

    #[test]
    fn budget_stop_sets_certified_gap() {
        let knobs = ThoughtSearchKnobs {
            max_nodes: 5,
            ..ThoughtSearchKnobs::default()
        };
        for (nodes_used, tokens_used, k) in [
            (5, 0, knobs),
            (0, 200_000, ThoughtSearchKnobs::default()),
            (
                3,
                0,
                ThoughtSearchKnobs {
                    max_nodes: 5,
                    max_branching: 4,
                    ..ThoughtSearchKnobs::default()
                },
            ),
        ] {
            match choose_expansion(
                &state(frontier(), nodes_used, tokens_used),
                ReasoningStrategy::Tree,
                &k,
            ) {
                ThoughtSearchAnswer::Stop {
                    best_node,
                    certified_gap,
                    gap,
                    path_digest,
                } => {
                    assert_eq!(best_node, 3);
                    assert!(certified_gap);
                    assert_eq!(gap, q32(0));
                    assert_eq!(
                        path_digest,
                        thought_path_digest(&[(1, q32(50)), (3, q32(90))])
                    );
                }
                other => panic!("expected stop, got {other:?}"),
            }
        }
    }

    #[test]
    fn golden_vector_replays_identically() {
        let knobs = ThoughtSearchKnobs {
            max_nodes: 5,
            ..ThoughtSearchKnobs::default()
        };
        let first = choose_expansion(
            &state(frontier(), 5, 0),
            ReasoningStrategy::Planning,
            &knobs,
        );
        for _ in 0..8 {
            assert_eq!(
                choose_expansion(
                    &state(frontier(), 5, 0),
                    ReasoningStrategy::Planning,
                    &knobs
                ),
                first
            );
        }
        let ThoughtSearchAnswer::Stop { path_digest, .. } = first else {
            panic!("expected stop");
        };
        // Pinned bytes: sha256("eg/thought-path/v1" NUL
        // `[[1,{"scale":"q32","value":50}],[3,{"scale":"q32","value":90}]]`).
        assert_eq!(
            path_digest,
            "sha256:bdbcd30e4b78b331f02001dc9b9699b9188cad0e5039be38dd28cb7973f62625"
        );
    }
}
