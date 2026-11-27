//! The bounded neighborhood walk behind `Method::GetNeighborhood`
//! (MEDULLA-DURABLE-KERNEL-R876.2).
//!
//! A breadth-first walk from one node to a depth of at most three. At each node the
//! neighbors sort by edge type (in the order the caller listed them), then by neighbor
//! degree descending, then by node id ascending, so equal calls return equal answers. The
//! walk keeps at most `per_node_limit` neighbors per node and `total_limit` neighbors
//! overall; ids the viewer already shows (`exclude`) are not counted. A node that keeps
//! fewer neighbors than it has visible gets a hidden count, and the cursor resumes at the
//! first neighbor of the root the walk dropped. The kernel is pure: it reads adjacency
//! through [`NeighborhoodSource`] and holds no lock of its own.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};

use medulla_types::contract::ServerErrorCode;

/// The deepest neighborhood a caller may ask for.
pub const MAX_NEIGHBORHOOD_DEPTH: u32 = 3;
/// The most neighbors one read may keep overall.
pub const MAX_NEIGHBORHOOD_TOTAL: u32 = 5_000;
/// The most ids a caller may list in `exclude`.
pub const MAX_NEIGHBORHOOD_EXCLUDE: usize = 5_000;

/// One link between a node and a neighbor, in one direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeighborLink {
    /// The id at the far end of the link.
    pub neighbor: String,
    /// `true` when the link runs from the walked node to `neighbor`.
    pub outgoing: bool,
    /// The relationship types of the parallel edges on this link (empty when untyped).
    pub types: BTreeSet<String>,
}

/// The adjacency the walk reads. The graph core implements it over its topology; a test
/// implements it over a plain map.
pub trait NeighborhoodSource {
    /// Whether the node exists (and is visible to the caller).
    fn has_node(&self, id: &str) -> bool;
    /// The links of `id`, one per (neighbor, direction).
    fn links(&self, id: &str) -> Vec<NeighborLink>;
    /// The number of edges touching `id`, both directions.
    fn degree(&self, id: &str) -> usize;
}

/// The arguments of one walk.
#[derive(Clone, Copy, Debug)]
pub struct NeighborhoodQuery<'a> {
    /// The node to walk from.
    pub root: &'a str,
    /// How many hops to walk, 1 to 3.
    pub depth: u32,
    /// The relationship types to follow, in rank order; empty follows every type.
    pub edge_types: &'a [String],
    /// The most neighbors one node keeps.
    pub per_node_limit: u32,
    /// The most neighbors the whole walk keeps (the root is not counted).
    pub total_limit: u32,
    /// Ids the viewer already shows; they are neither kept nor counted as hidden.
    pub exclude: &'a [String],
    /// How many of the root's sorted neighbors earlier pages already delivered.
    pub skip: usize,
}

/// Why a walk is refused; each variant is one registered refusal code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeighborhoodRefusal {
    /// The depth is outside 1 to 3.
    DepthExceeded,
    /// `per_node_limit` or `total_limit` is zero.
    LimitRequired,
    /// `total_limit` is above 5,000.
    LimitExceeded,
    /// `exclude` lists more than 5,000 ids.
    ExcludeTooLarge,
    /// The root does not exist.
    NodeNotFound,
}

impl NeighborhoodRefusal {
    /// The registered refusal code of this refusal.
    pub fn code(self) -> ServerErrorCode {
        match self {
            Self::DepthExceeded => ServerErrorCode::NeighborhoodDepthExceeded,
            Self::LimitRequired => ServerErrorCode::NeighborhoodLimitRequired,
            Self::LimitExceeded => ServerErrorCode::NeighborhoodLimitExceeded,
            Self::ExcludeTooLarge => ServerErrorCode::NeighborhoodExcludeTooLarge,
            Self::NodeNotFound => ServerErrorCode::NodeNotFound,
        }
    }
}

/// One edge of the walk, in its stored direction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NeighborhoodEdge {
    /// The edge's source node id.
    pub source: String,
    /// The edge's target node id.
    pub target: String,
}

/// What one walk found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NeighborhoodWalk {
    /// The root first, then every kept neighbor in the order the walk reached it.
    pub nodes: Vec<String>,
    /// Every link among the walk's nodes and the excluded ids it touched.
    pub edges: Vec<NeighborhoodEdge>,
    /// The nodes that kept fewer neighbors than they have, with how many stay hidden.
    pub hidden: Vec<(String, u64)>,
    /// Where the next page of the root's neighbors starts, when the root dropped any.
    pub next_skip: Option<usize>,
    /// Whether any neighbor stays hidden.
    pub truncated: bool,
}

/// Check a query's bounds (MEDULLA-DURABLE-KERNEL-R876.4), before any read.
pub fn validate(query: &NeighborhoodQuery<'_>) -> Result<(), NeighborhoodRefusal> {
    if query.depth == 0 || query.depth > MAX_NEIGHBORHOOD_DEPTH {
        return Err(NeighborhoodRefusal::DepthExceeded);
    }
    if query.per_node_limit == 0 || query.total_limit == 0 {
        return Err(NeighborhoodRefusal::LimitRequired);
    }
    if query.total_limit > MAX_NEIGHBORHOOD_TOTAL {
        return Err(NeighborhoodRefusal::LimitExceeded);
    }
    if query.exclude.len() > MAX_NEIGHBORHOOD_EXCLUDE {
        return Err(NeighborhoodRefusal::ExcludeTooLarge);
    }
    Ok(())
}

/// The audit line of one neighborhood read (MEDULLA-DURABLE-KERNEL-R876.5): the node id, the
/// depth, the node and edge counts and the truncation flag. It names no property value.
pub fn audit_line(node_id: &str, depth: u32, nodes: usize, edges: usize, truncated: bool) -> String {
    format!(
        "GET_NEIGHBORHOOD|{node_id}|depth={depth}|nodes={nodes}|edges={edges}|truncated={truncated}"
    )
}

/// The cursor that resumes at `skip` of the root's sorted neighbors, valid only at `revision`.
pub fn encode_cursor(revision: u64, skip: usize) -> String {
    format!("{revision}.{skip}")
}

/// The `(revision, skip)` a cursor carries, or `None` when it is not a cursor this walk issued.
pub fn decode_cursor(cursor: &str) -> Option<(u64, usize)> {
    let (revision, skip) = cursor.split_once('.')?;
    Some((revision.parse().ok()?, skip.parse().ok()?))
}

/// One neighbor of the walked node, with the ranking facts the sort needs.
struct Candidate {
    id: String,
    rank: usize,
    degree: usize,
}

/// Run the walk.
pub fn walk<S: NeighborhoodSource + ?Sized>(
    source: &S,
    query: &NeighborhoodQuery<'_>,
) -> Result<NeighborhoodWalk, NeighborhoodRefusal> {
    validate(query)?;
    if !source.has_node(query.root) {
        return Err(NeighborhoodRefusal::NodeNotFound);
    }
    let exclude: HashSet<&str> = query.exclude.iter().map(String::as_str).collect();
    let rank_of: HashMap<&str, usize> = query
        .edge_types
        .iter()
        .enumerate()
        .rev()
        .map(|(rank, kind)| (kind.as_str(), rank))
        .collect();
    let mut out = NeighborhoodWalk {
        nodes: vec![query.root.to_string()],
        ..NeighborhoodWalk::default()
    };
    let mut visited: HashSet<String> = HashSet::from([query.root.to_string()]);
    let mut seen_edges: HashSet<NeighborhoodEdge> = HashSet::new();
    let mut kept_total: usize = 0;
    let mut frontier: Vec<String> = vec![query.root.to_string()];
    for level in 0..=query.depth {
        let expands = level < query.depth;
        let mut next_frontier: Vec<String> = Vec::new();
        for node in &frontier {
            let links = followed_links(source, node, query.edge_types, &rank_of);
            record_edges(node, &links, &visited, &exclude, &mut seen_edges, &mut out);
            let mut candidates = candidates_of(source, &links, &visited, &exclude, expands);
            candidates.sort_by(|a, b| {
                (a.rank, Reverse(a.degree), &a.id).cmp(&(b.rank, Reverse(b.degree), &b.id))
            });
            let is_root = level == 0;
            let skipped = if is_root {
                query.skip.min(candidates.len())
            } else {
                0
            };
            let budget = (query.per_node_limit as usize)
                .min((query.total_limit as usize).saturating_sub(kept_total));
            let keep = if expands {
                budget.min(candidates.len() - skipped)
            } else {
                0
            };
            for candidate in candidates.iter().skip(skipped).take(keep) {
                visited.insert(candidate.id.clone());
                out.nodes.push(candidate.id.clone());
                next_frontier.push(candidate.id.clone());
                kept_total += 1;
            }
            for (link, _) in &links {
                if visited.contains(&link.neighbor) {
                    push_edge(node, link, &mut seen_edges, &mut out.edges);
                }
            }
            let hidden = candidates.len() - skipped - keep;
            if !expands {
                // A last-level node is never walked; its candidates were not ranked, so
                // count every unseen neighbor it has.
                let unseen = unseen_neighbors(&links, &visited, &exclude);
                if unseen > 0 {
                    out.hidden.push((node.clone(), unseen as u64));
                }
            } else if hidden > 0 {
                out.hidden.push((node.clone(), hidden as u64));
                if is_root {
                    out.next_skip = Some(skipped + keep);
                }
            }
        }
        frontier = next_frontier;
    }
    out.truncated = !out.hidden.is_empty();
    Ok(out)
}

/// The links of `node` the query follows, with each link's best (lowest) type rank.
fn followed_links<S: NeighborhoodSource + ?Sized>(
    source: &S,
    node: &str,
    edge_types: &[String],
    rank_of: &HashMap<&str, usize>,
) -> Vec<(NeighborLink, usize)> {
    source
        .links(node)
        .into_iter()
        .filter_map(|link| {
            if edge_types.is_empty() {
                return Some((link, 0));
            }
            let rank = link
                .types
                .iter()
                .filter_map(|kind| rank_of.get(kind.as_str()).copied())
                .min()?;
            Some((link, rank))
        })
        .collect()
}

fn candidates_of<S: NeighborhoodSource + ?Sized>(
    source: &S,
    links: &[(NeighborLink, usize)],
    visited: &HashSet<String>,
    exclude: &HashSet<&str>,
    rank_degrees: bool,
) -> Vec<Candidate> {
    let mut best: HashMap<&str, usize> = HashMap::new();
    for (link, rank) in links {
        if visited.contains(&link.neighbor) || exclude.contains(link.neighbor.as_str()) {
            continue;
        }
        best.entry(link.neighbor.as_str())
            .and_modify(|current| *current = (*current).min(*rank))
            .or_insert(*rank);
    }
    best.into_iter()
        .map(|(id, rank)| Candidate {
            id: id.to_string(),
            rank,
            degree: if rank_degrees { source.degree(id) } else { 0 },
        })
        .collect()
}

fn unseen_neighbors(
    links: &[(NeighborLink, usize)],
    visited: &HashSet<String>,
    exclude: &HashSet<&str>,
) -> usize {
    links
        .iter()
        .map(|(link, _)| link.neighbor.as_str())
        .filter(|id| !visited.contains(*id) && !exclude.contains(id))
        .collect::<HashSet<_>>()
        .len()
}

/// Record the links from `node` to ids the viewer already has: excluded ids and nodes the
/// walk reached earlier. Links to kept neighbors are recorded once they are kept.
fn record_edges(
    node: &str,
    links: &[(NeighborLink, usize)],
    visited: &HashSet<String>,
    exclude: &HashSet<&str>,
    seen: &mut HashSet<NeighborhoodEdge>,
    out: &mut NeighborhoodWalk,
) {
    for (link, _) in links {
        if exclude.contains(link.neighbor.as_str()) || visited.contains(&link.neighbor) {
            push_edge(node, link, seen, &mut out.edges);
        }
    }
}

fn push_edge(
    node: &str,
    link: &NeighborLink,
    seen: &mut HashSet<NeighborhoodEdge>,
    edges: &mut Vec<NeighborhoodEdge>,
) {
    let edge = if link.outgoing {
        NeighborhoodEdge {
            source: node.to_string(),
            target: link.neighbor.clone(),
        }
    } else {
        NeighborhoodEdge {
            source: link.neighbor.clone(),
            target: node.to_string(),
        }
    };
    if seen.insert(edge.clone()) {
        edges.push(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A plain adjacency map: `(source, target, type)` triples.
    struct Fixture {
        nodes: BTreeSet<String>,
        edges: Vec<(String, String, String)>,
    }

    impl Fixture {
        fn new(edges: &[(&str, &str, &str)]) -> Self {
            let mut nodes = BTreeSet::new();
            for (s, t, _) in edges {
                nodes.insert(s.to_string());
                nodes.insert(t.to_string());
            }
            Self {
                nodes,
                edges: edges
                    .iter()
                    .map(|(s, t, k)| (s.to_string(), t.to_string(), k.to_string()))
                    .collect(),
            }
        }
    }

    impl NeighborhoodSource for Fixture {
        fn has_node(&self, id: &str) -> bool {
            self.nodes.contains(id)
        }
        fn links(&self, id: &str) -> Vec<NeighborLink> {
            let mut by_link: BTreeMap<(String, bool), BTreeSet<String>> = BTreeMap::new();
            for (s, t, k) in &self.edges {
                if s == id {
                    by_link
                        .entry((t.clone(), true))
                        .or_default()
                        .insert(k.clone());
                }
                if t == id {
                    by_link
                        .entry((s.clone(), false))
                        .or_default()
                        .insert(k.clone());
                }
            }
            by_link
                .into_iter()
                .map(|((neighbor, outgoing), types)| NeighborLink {
                    neighbor,
                    outgoing,
                    types,
                })
                .collect()
        }
        fn degree(&self, id: &str) -> usize {
            self.edges
                .iter()
                .filter(|(s, t, _)| s == id || t == id)
                .count()
        }
    }

    fn query<'a>(root: &'a str, depth: u32, per: u32, total: u32) -> NeighborhoodQuery<'a> {
        NeighborhoodQuery {
            root,
            depth,
            edge_types: &[],
            per_node_limit: per,
            total_limit: total,
            exclude: &[],
            skip: 0,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R876.2
    #[test]
    fn order_limits_and_hidden_counts_are_deterministic() {
        // `hub` links to a..e. `c` has two more edges (degree 3), so it sorts first among
        // equal-ranked neighbors; the rest sort by id.
        let source = Fixture::new(&[
            ("hub", "a", "KNOWS"),
            ("hub", "b", "KNOWS"),
            ("hub", "c", "KNOWS"),
            ("hub", "d", "OWNS"),
            ("hub", "e", "KNOWS"),
            ("c", "x", "KNOWS"),
            ("c", "y", "KNOWS"),
        ]);
        let types = vec!["OWNS".to_string(), "KNOWS".to_string()];
        let mut q = query("hub", 1, 3, 10);
        q.edge_types = &types;
        let first = walk(&source, &q).expect("walk");
        // OWNS ranks before KNOWS; then degree descending (c); then id ascending (a).
        assert_eq!(first.nodes, vec!["hub", "d", "c", "a"]);
        assert_eq!(first.hidden.first(), Some(&("hub".to_string(), 2)));
        assert_eq!(first.next_skip, Some(3));
        assert!(first.truncated);
        assert_eq!(walk(&source, &q).expect("again"), first, "equal calls, equal answers");

        // The cursor resumes at the first dropped neighbor.
        let mut resumed = q;
        resumed.skip = 3;
        let page = walk(&source, &resumed).expect("page");
        assert_eq!(page.nodes, vec!["hub", "b", "e"]);
        assert_eq!(page.next_skip, None);
        assert!(!page.truncated || page.hidden.iter().all(|(id, _)| id != "hub"));

        // A total limit caps the neighbors kept overall; excluded ids are not counted.
        let excluded = vec!["d".to_string(), "c".to_string()];
        let mut capped = query("hub", 1, 10, 2);
        capped.exclude = &excluded;
        let walked = walk(&source, &capped).expect("capped");
        assert_eq!(walked.nodes, vec!["hub", "a", "b"]);
        assert_eq!(walked.hidden, vec![("hub".to_string(), 1)]);
        assert!(walked
            .edges
            .contains(&NeighborhoodEdge {
                source: "hub".into(),
                target: "c".into()
            }));

        // Depth 2 walks on from kept neighbors; last-level nodes report what stays hidden.
        let deep = walk(&source, &query("hub", 2, 10, 10)).expect("deep");
        assert!(deep.nodes.contains(&"x".to_string()));
        let shallow = walk(&source, &query("hub", 1, 10, 10)).expect("shallow");
        assert!(shallow.hidden.contains(&("c".to_string(), 2)));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R876.4
    #[test]
    fn bounds_and_cursor_are_checked_before_any_read() {
        let source = Fixture::new(&[("a", "b", "K")]);
        assert_eq!(
            walk(&source, &query("a", 0, 1, 1)).unwrap_err(),
            NeighborhoodRefusal::DepthExceeded
        );
        assert_eq!(
            walk(&source, &query("a", 4, 1, 1)).unwrap_err(),
            NeighborhoodRefusal::DepthExceeded
        );
        assert_eq!(
            walk(&source, &query("a", 1, 0, 1)).unwrap_err(),
            NeighborhoodRefusal::LimitRequired
        );
        assert_eq!(
            walk(&source, &query("a", 1, 1, 5_001)).unwrap_err(),
            NeighborhoodRefusal::LimitExceeded
        );
        assert_eq!(
            walk(&source, &query("zz", 1, 1, 1)).unwrap_err(),
            NeighborhoodRefusal::NodeNotFound
        );
        let big: Vec<String> = (0..=MAX_NEIGHBORHOOD_EXCLUDE).map(|i| i.to_string()).collect();
        let mut q = query("a", 1, 1, 1);
        q.exclude = &big;
        assert_eq!(walk(&source, &q).unwrap_err(), NeighborhoodRefusal::ExcludeTooLarge);
        assert_eq!(decode_cursor(&encode_cursor(7, 50)), Some((7, 50)));
        assert_eq!(decode_cursor("garbage"), None);
    }
}
