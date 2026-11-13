//! Value-of-information ordering of a research citation walk
//! (MEDULLA-DECISION-ENGINE-R705.2).
//!
//! Pure and deterministic. The expected value of reading a source is
//! `p_useful * gain - cost`, in Q32 fixed point. The frontier is a binary heap
//! ordered by expected value, highest first, then by source id, lowest first.
//! The walk stops when the node budget is spent, when the best expected value
//! is at or below the floor, or when no reachable source is left. The number of
//! results never bounds the walk: only the node budget does.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// One Q32 unit: `1.0`.
pub const Q32_ONE: i64 = 1 << 32;

/// What a caller knows about one source before reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNode {
    pub source_id: String,
    /// Probability that reading it helps, on Q32 (`0..=Q32_ONE`).
    pub p_useful_q32: i64,
    /// Value of a helpful read, on Q32.
    pub gain_q32: i64,
    /// Cost of reading it, on Q32.
    pub cost_q32: i64,
    /// The sources it cites.
    pub links: Vec<String>,
}

/// The registry knobs the walk reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontierLimits {
    /// `research.node_budget`: most sources the walk reads.
    pub node_budget: u64,
    /// `research.depth_limit`: most links from a seed.
    pub depth_limit: u8,
    /// `research.value_floor_micro`: stop at or below this expected value.
    pub value_floor_micro: u64,
}

/// Why the walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierStop {
    /// Every reachable source within the depth limit was read.
    Exhausted,
    /// The node budget was spent.
    NodeBudget,
    /// The best remaining expected value is at or below the floor.
    ValueFloor,
}

/// One read, in the order the walk chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierRead {
    pub source_id: String,
    pub depth: u8,
    pub expected_value_q32: i64,
}

/// The ordered reads and the reason the walk stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierWalk {
    pub reads: Vec<FrontierRead>,
    pub stop: FrontierStop,
}

/// `p_useful * gain - cost` on Q32.
pub fn expected_value_q32(node: &SourceNode) -> i64 {
    let p = i128::from(node.p_useful_q32.clamp(0, Q32_ONE));
    let gain = i128::from(node.gain_q32);
    let product = (p * gain) >> 32;
    let value = product - i128::from(node.cost_q32);
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn micro_to_q32(micro: u64) -> i64 {
    ((i128::from(micro) << 32) / 1_000_000) as i64
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    value: i64,
    source_id: String,
    depth: u8,
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value
            .cmp(&other.value)
            .then_with(|| other.source_id.cmp(&self.source_id))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Walk the citation links from `seeds` in expected-value order.
///
/// A seed or a link naming a source absent from `nodes` is skipped. A source
/// more than `limits.depth_limit` links from a seed is skipped.
pub fn expand_frontier(
    seeds: &[String],
    nodes: &BTreeMap<String, SourceNode>,
    limits: &FrontierLimits,
) -> FrontierWalk {
    let floor = micro_to_q32(limits.value_floor_micro);
    let mut heap: BinaryHeap<Entry> = BinaryHeap::new();
    let mut queued: BTreeSet<&str> = BTreeSet::new();
    let mut push = |heap: &mut BinaryHeap<Entry>, id: &str, depth: u8| {
        if let Some(node) = nodes.get(id) {
            if queued.insert(node.source_id.as_str()) {
                heap.push(Entry {
                    value: expected_value_q32(node),
                    source_id: node.source_id.clone(),
                    depth,
                });
            }
        }
    };
    for seed in seeds {
        push(&mut heap, seed, 0);
    }
    let mut reads = Vec::new();
    let stop = loop {
        if reads.len() as u64 >= limits.node_budget {
            break FrontierStop::NodeBudget;
        }
        let Some(best) = heap.pop() else {
            break FrontierStop::Exhausted;
        };
        if best.value <= floor {
            break FrontierStop::ValueFloor;
        }
        if best.depth < limits.depth_limit {
            if let Some(node) = nodes.get(&best.source_id) {
                for link in &node.links {
                    push(&mut heap, link, best.depth + 1);
                }
            }
        }
        reads.push(FrontierRead {
            source_id: best.source_id,
            depth: best.depth,
            expected_value_q32: best.value,
        });
    };
    FrontierWalk { reads, stop }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: FrontierLimits = FrontierLimits {
        node_budget: 1_000,
        depth_limit: 3,
        value_floor_micro: 0,
    };

    fn node(id: &str, p_pct: i64, gain: i64, cost: i64, links: &[&str]) -> SourceNode {
        SourceNode {
            source_id: id.into(),
            p_useful_q32: Q32_ONE * p_pct / 100,
            gain_q32: gain * Q32_ONE,
            cost_q32: cost * Q32_ONE,
            links: links.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn graph(nodes: Vec<SourceNode>) -> BTreeMap<String, SourceNode> {
        nodes
            .into_iter()
            .map(|n| (n.source_id.clone(), n))
            .collect()
    }

    fn ids(walk: &FrontierWalk) -> Vec<&str> {
        walk.reads.iter().map(|r| r.source_id.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R705.2
    #[test]
    fn heap_order_node_budget_and_stop_rule() {
        // 10 nodes: value descending, then source id ascending.
        let ten = graph(vec![
            node(
                "s",
                100,
                10,
                0,
                &["a", "b", "c", "d", "e", "f", "g", "h", "i"],
            ),
            node("a", 50, 10, 1, &[]),
            node("b", 100, 10, 1, &[]),
            node("c", 50, 10, 1, &[]),
            node("d", 100, 20, 1, &[]),
            node("e", 10, 10, 0, &[]),
            node("f", 50, 10, 1, &[]),
            node("g", 100, 5, 1, &[]),
            node("h", 20, 10, 0, &[]),
            node("i", 90, 10, 1, &[]),
        ]);
        let walk = expand_frontier(&["s".to_string()], &ten, &LIMITS);
        assert_eq!(walk.stop, FrontierStop::Exhausted);
        // Values: s 10, d 19, b 9, i 8, g 4, h 2, a/c/f 4 (50% of 10 less 1), e 1.
        assert_eq!(
            ids(&walk),
            ["s", "d", "b", "i", "a", "c", "f", "g", "h", "e"]
        );
        assert!(walk
            .reads
            .windows(2)
            .skip(1)
            .all(|w| w[0].expected_value_q32 >= w[1].expected_value_q32));

        // 1,000 nodes: the node budget stops the walk, not the result count.
        let wide = star(999);
        let capped = FrontierLimits {
            node_budget: 10,
            ..LIMITS
        };
        let walk = expand_frontier(&["root".to_string()], &wide, &capped);
        assert_eq!(walk.stop, FrontierStop::NodeBudget);
        assert_eq!(walk.reads.len(), 10);
        let full = expand_frontier(&["root".to_string()], &wide, &LIMITS);
        assert_eq!(full.stop, FrontierStop::Exhausted);
        assert_eq!(full.reads.len(), 1_000);

        // 100,000 nodes: the default budget of 1,000 bounds the work, and a
        // raised budget reads them all.
        let huge = star(99_999);
        let walk = expand_frontier(&["root".to_string()], &huge, &LIMITS);
        assert_eq!(
            (walk.stop, walk.reads.len()),
            (FrontierStop::NodeBudget, 1_000)
        );
        let all = FrontierLimits {
            node_budget: 100_000,
            ..LIMITS
        };
        let walk = expand_frontier(&["root".to_string()], &huge, &all);
        assert_eq!(
            (walk.stop, walk.reads.len()),
            (FrontierStop::Exhausted, 100_000)
        );

        // Depth limit: a chain of four links is cut after depth 3.
        let chain = graph(vec![
            node("c0", 100, 10, 0, &["c1"]),
            node("c1", 100, 10, 0, &["c2"]),
            node("c2", 100, 10, 0, &["c3"]),
            node("c3", 100, 10, 0, &["c4"]),
            node("c4", 100, 10, 0, &[]),
        ]);
        let walk = expand_frontier(&["c0".to_string()], &chain, &LIMITS);
        assert_eq!(ids(&walk), ["c0", "c1", "c2", "c3"]);
        assert_eq!(walk.stop, FrontierStop::Exhausted);

        // Value floor: the walk stops at the first expected value at or below it.
        let floor = FrontierLimits {
            value_floor_micro: 5_000_000,
            ..LIMITS
        };
        let walk = expand_frontier(&["s".to_string()], &ten, &floor);
        assert_eq!(walk.stop, FrontierStop::ValueFloor);
        assert_eq!(ids(&walk), ["s", "d", "b", "i"]);

        // A source absent from the graph is skipped; a zero-value seed stops at once.
        let walk = expand_frontier(&["missing".to_string()], &ten, &LIMITS);
        assert_eq!((walk.reads.len(), walk.stop), (0, FrontierStop::Exhausted));
        let dead = graph(vec![node("z", 100, 10, 10, &[])]);
        let walk = expand_frontier(&["z".to_string()], &dead, &LIMITS);
        assert_eq!((walk.reads.len(), walk.stop), (0, FrontierStop::ValueFloor));
    }

    /// One root citing `children` sources of equal value; ids sort numerically
    /// because they are zero padded.
    fn star(children: usize) -> BTreeMap<String, SourceNode> {
        let ids: Vec<String> = (0..children).map(|i| format!("n{i:06}")).collect();
        let mut nodes = vec![SourceNode {
            links: ids.clone(),
            ..node("root", 100, 10, 0, &[])
        }];
        nodes.extend(ids.iter().map(|id| node(id, 50, 10, 1, &[])));
        graph(nodes)
    }
}
