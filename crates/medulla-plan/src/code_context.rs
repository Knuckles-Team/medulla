//! Code context and symptom diagnosis over a repository symbol index.
//!
//! [`code_context`] resolves an anchor symbol (exact qualified name, else
//! suffix match), expands callers and callees breadth first up to a depth and
//! node cap, and ranks hubs of the visited subgraph by directed betweenness
//! centrality through [`rank_hubs`]. [`diagnose`] walks operations links
//! backward from a symptom node and orders the causal chain by stage, then
//! time. Both are read-only; evidence is carried inline in the answers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use medulla_graph_algos::{graph_algos::betweenness_centrality, graph_algos::AdjacencyGraph};

/// A directed caller-to-callee edge in a resolved code subgraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeEdge {
    pub caller: String,
    pub callee: String,
}

impl CodeEdge {
    /// Construct one edge from the canonical symbol identifiers.
    pub fn new(caller: impl Into<String>, callee: impl Into<String>) -> Self {
        Self {
            caller: caller.into(),
            callee: callee.into(),
        }
    }
}

/// A code symbol ranked as a hub of the supplied subgraph.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeHub {
    /// Canonical symbol identifier.
    pub symbol: String,
    /// Directed betweenness centrality over the supplied code subgraph.
    pub centrality: f64,
}

/// Rank the most central symbols in a resolved caller/callee subgraph.
///
/// The graph kernel supplies sorted node identities and deterministic Brandes
/// traversal. Ranking then sorts by descending centrality and ascending symbol
/// identifier, so equal-score hubs do not change order between runs. `limit`
/// bounds the returned result and zero returns an empty vector without building
/// a graph. Duplicate edges are merged by [`AdjacencyGraph`].
pub fn rank_hubs(edges: &[CodeEdge], limit: usize) -> Vec<CodeHub> {
    if limit == 0 || edges.is_empty() {
        return Vec::new();
    }

    let graph = AdjacencyGraph::from_edges(
        edges
            .iter()
            .map(|edge| (edge.caller.clone(), edge.callee.clone(), 1.0)),
    );
    let mut hubs: Vec<CodeHub> = betweenness_centrality(&graph, true)
        .into_iter()
        .map(|(symbol, centrality)| CodeHub { symbol, centrality })
        .collect();
    hubs.sort_by(|left, right| {
        right
            .centrality
            .total_cmp(&left.centrality)
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    hubs.truncate(limit);
    hubs
}

/// Default breadth-first depth (`knob code.context_depth`).
pub const DEFAULT_CONTEXT_DEPTH: u32 = 3;
/// Largest accepted depth; deeper requests refuse with `CODE_DEPTH_EXCEEDED`.
pub const MAX_CONTEXT_DEPTH: u32 = 6;
/// Default visited-node cap (`knob code.max_nodes`).
pub const DEFAULT_MAX_NODES: usize = 512;
/// Smallest accepted node cap.
pub const MIN_MAX_NODES: usize = 32;
/// Largest accepted node cap.
pub const MAX_MAX_NODES: usize = 8_192;
/// Fixed backward walk depth of [`diagnose`].
pub const DIAGNOSE_DEPTH: u32 = 3;

/// Read-only view of the repository symbol index.
#[derive(Debug, Clone, Default)]
pub struct CodeIndex {
    /// Canonical qualified symbol names across every indexed repository.
    pub symbols: Vec<String>,
    /// Caller-to-callee edges across repositories.
    pub edges: Vec<CodeEdge>,
    /// Repositories whose index is stale or partial.
    pub missing_repositories: Vec<String>,
}

/// One code-context request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeContextRequest {
    pub anchor: String,
    pub depth: Option<u32>,
    pub max_nodes: Option<usize>,
}

/// Answer status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeContextStatus {
    Ok,
    Ambiguous,
    Degraded,
}

impl CodeContextStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Ambiguous => "ambiguous",
            Self::Degraded => "degraded",
        }
    }
}

/// A visited symbol and its hop count from the anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextNode {
    pub symbol: String,
    pub hops: u32,
}

/// A hub with its centrality inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextHub {
    pub symbol: String,
    pub in_degree: usize,
    pub out_degree: usize,
    pub score: f64,
}

/// The code-context answer; evidence is inline.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeContextAnswer {
    pub status: CodeContextStatus,
    pub anchor: Option<String>,
    pub candidates: Vec<String>,
    pub missing_repositories: Vec<String>,
    pub nodes: Vec<ContextNode>,
    pub edges: Vec<CodeEdge>,
    pub hubs: Vec<ContextHub>,
}

/// Typed refusals of the code-context methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeContextError {
    DepthExceeded { depth: u32 },
    AnchorUnknown { anchor: String },
}

impl std::fmt::Display for CodeContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DepthExceeded { depth } => write!(
                f,
                "CODE_DEPTH_EXCEEDED: depth {depth} exceeds {MAX_CONTEXT_DEPTH}"
            ),
            Self::AnchorUnknown { anchor } => {
                write!(f, "CODE_ANCHOR_UNKNOWN: no symbol matches '{anchor}'")
            }
        }
    }
}

impl std::error::Error for CodeContextError {}

impl CodeContextError {
    /// Stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DepthExceeded { .. } => "CODE_DEPTH_EXCEEDED",
            Self::AnchorUnknown { .. } => "CODE_ANCHOR_UNKNOWN",
        }
    }
}

fn resolve_anchor(index: &CodeIndex, anchor: &str) -> Vec<String> {
    if index.symbols.iter().any(|symbol| symbol == anchor) {
        return vec![anchor.to_string()];
    }
    let matches: BTreeSet<String> = index
        .symbols
        .iter()
        .filter(|symbol| symbol.ends_with(anchor))
        .cloned()
        .collect();
    matches.into_iter().collect()
}

/// Build the bounded code context around `request.anchor`.
pub fn code_context(
    index: &CodeIndex,
    request: &CodeContextRequest,
) -> Result<CodeContextAnswer, CodeContextError> {
    let depth = request.depth.unwrap_or(DEFAULT_CONTEXT_DEPTH);
    if depth > MAX_CONTEXT_DEPTH {
        return Err(CodeContextError::DepthExceeded { depth });
    }
    let depth = depth.max(1);
    let max_nodes = request
        .max_nodes
        .unwrap_or(DEFAULT_MAX_NODES)
        .clamp(MIN_MAX_NODES, MAX_MAX_NODES);
    let mut missing = index.missing_repositories.clone();
    missing.sort();
    missing.dedup();
    let candidates = resolve_anchor(index, &request.anchor);
    let anchor = match candidates.as_slice() {
        [] => {
            return Err(CodeContextError::AnchorUnknown {
                anchor: request.anchor.clone(),
            })
        }
        [one] => one.clone(),
        _ => {
            return Ok(CodeContextAnswer {
                status: CodeContextStatus::Ambiguous,
                anchor: None,
                candidates,
                missing_repositories: missing,
                nodes: Vec::new(),
                edges: Vec::new(),
                hubs: Vec::new(),
            })
        }
    };

    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in &index.edges {
        adjacency
            .entry(edge.caller.as_str())
            .or_default()
            .insert(edge.callee.as_str());
        adjacency
            .entry(edge.callee.as_str())
            .or_default()
            .insert(edge.caller.as_str());
    }
    let mut hops: BTreeMap<String, u32> = BTreeMap::new();
    hops.insert(anchor.clone(), 0);
    let mut queue = VecDeque::from([(anchor.clone(), 0u32)]);
    'walk: while let Some((symbol, hop)) = queue.pop_front() {
        if hop >= depth {
            continue;
        }
        for next in adjacency.get(symbol.as_str()).into_iter().flatten() {
            if hops.contains_key(*next) {
                continue;
            }
            if hops.len() >= max_nodes {
                break 'walk;
            }
            hops.insert((*next).to_string(), hop + 1);
            queue.push_back(((*next).to_string(), hop + 1));
        }
    }

    let mut edges: Vec<CodeEdge> = index
        .edges
        .iter()
        .filter(|edge| hops.contains_key(&edge.caller) && hops.contains_key(&edge.callee))
        .cloned()
        .collect();
    edges.sort_by(|a, b| (&a.caller, &a.callee).cmp(&(&b.caller, &b.callee)));
    edges.dedup();
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut out_degree: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &edges {
        *out_degree.entry(edge.caller.as_str()).or_default() += 1;
        *in_degree.entry(edge.callee.as_str()).or_default() += 1;
    }
    let hubs = rank_hubs(&edges, hops.len())
        .into_iter()
        .map(|hub| ContextHub {
            in_degree: in_degree.get(hub.symbol.as_str()).copied().unwrap_or(0),
            out_degree: out_degree.get(hub.symbol.as_str()).copied().unwrap_or(0),
            score: hub.centrality,
            symbol: hub.symbol,
        })
        .collect();
    let mut nodes: Vec<ContextNode> = hops
        .into_iter()
        .map(|(symbol, hops)| ContextNode { symbol, hops })
        .collect();
    nodes.sort_by(|a, b| a.hops.cmp(&b.hops).then_with(|| a.symbol.cmp(&b.symbol)));
    Ok(CodeContextAnswer {
        status: if missing.is_empty() {
            CodeContextStatus::Ok
        } else {
            CodeContextStatus::Degraded
        },
        anchor: Some(anchor),
        candidates: Vec::new(),
        missing_repositories: missing,
        nodes,
        edges,
        hubs,
    })
}

/// Causal stage of an operations node, in chain order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpsStage {
    Change,
    Deploy,
    Config,
    Dependency,
    Resource,
    Symptom,
}

/// Stage by ontology label (`infrastructure-v1.ttl`).
pub const OPS_STAGE_BY_LABEL: &[(&str, OpsStage)] = &[
    ("Change", OpsStage::Change),
    ("Deployment", OpsStage::Deploy),
    ("ConfigItem", OpsStage::Config),
    ("Service", OpsStage::Dependency),
    ("Host", OpsStage::Resource),
    ("HealthAnomaly", OpsStage::Symptom),
];

fn stage_of(label: &str) -> Option<OpsStage> {
    OPS_STAGE_BY_LABEL
        .iter()
        .find(|(name, _)| *name == label)
        .map(|(_, stage)| *stage)
}

/// An operations node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpsNode {
    pub id: String,
    pub label: String,
    pub at_ms: i64,
}

/// Kind of an operations link walked by [`diagnose`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpsLinkKind {
    DependsOn,
    Change,
}

/// A link from `from` to `to`; [`diagnose`] walks it backward from `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpsLink {
    pub from: String,
    pub to: String,
    pub kind: OpsLinkKind,
    pub evidence_ids: Vec<String>,
}

/// A chain entry with its causal stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalNode {
    pub id: String,
    pub stage: OpsStage,
    pub at_ms: i64,
    pub hops: u32,
}

/// A walked link with the evidence ids of the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalLink {
    pub from: String,
    pub to: String,
    pub evidence_ids: Vec<String>,
}

/// The diagnosis of one symptom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymptomDiagnosis {
    pub symptom: String,
    pub chain: Vec<CausalNode>,
    pub links: Vec<CausalLink>,
}

/// Walk `dependsOn` and change links backward from a `HealthAnomaly` symptom
/// to depth 3 and order the chain by stage, then time.
pub fn diagnose(
    nodes: &[OpsNode],
    links: &[OpsLink],
    symptom: &str,
) -> Result<SymptomDiagnosis, CodeContextError> {
    let by_id: BTreeMap<&str, &OpsNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let Some(start) = by_id
        .get(symptom)
        .filter(|node| stage_of(&node.label) == Some(OpsStage::Symptom))
    else {
        return Err(CodeContextError::AnchorUnknown {
            anchor: symptom.to_string(),
        });
    };
    let mut hops: BTreeMap<&str, u32> = BTreeMap::from([(start.id.as_str(), 0)]);
    let mut walked: Vec<CausalLink> = Vec::new();
    let mut queue = VecDeque::from([(start.id.as_str(), 0u32)]);
    while let Some((id, hop)) = queue.pop_front() {
        if hop >= DIAGNOSE_DEPTH {
            continue;
        }
        for link in links.iter().filter(|link| link.to == id) {
            let Some(cause) = by_id.get(link.from.as_str()) else {
                continue;
            };
            if stage_of(&cause.label).is_none() {
                continue;
            }
            walked.push(CausalLink {
                from: link.from.clone(),
                to: link.to.clone(),
                evidence_ids: link.evidence_ids.clone(),
            });
            if !hops.contains_key(cause.id.as_str()) {
                hops.insert(cause.id.as_str(), hop + 1);
                queue.push_back((cause.id.as_str(), hop + 1));
            }
        }
    }
    let mut chain: Vec<CausalNode> = hops
        .iter()
        .filter_map(|(id, hop)| {
            let node = by_id.get(id)?;
            Some(CausalNode {
                id: node.id.clone(),
                stage: stage_of(&node.label)?,
                at_ms: node.at_ms,
                hops: *hop,
            })
        })
        .collect();
    chain.sort_by(|a, b| {
        a.stage
            .cmp(&b.stage)
            .then(a.at_ms.cmp(&b.at_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    walked.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    walked.dedup();
    Ok(SymptomDiagnosis {
        symptom: symptom.to_string(),
        chain,
        links: walked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_hubs_is_deterministic_and_tie_breaks_symbols() {
        let edges = vec![
            CodeEdge::new("repo/a.rs::main", "repo/b.rs::parse"),
            CodeEdge::new("repo/b.rs::parse", "repo/c.rs::normalize"),
            CodeEdge::new("repo/c.rs::normalize", "repo/d.rs::emit"),
        ];

        let first = rank_hubs(&edges, 2);
        let second = rank_hubs(&edges, 2);

        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|hub| hub.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["repo/b.rs::parse", "repo/c.rs::normalize"]
        );
        assert_eq!(first[0].centrality, first[1].centrality);
        assert!(first[0].centrality > 0.0);
    }

    #[test]
    fn rank_hubs_refuses_empty_or_zero_budget_without_partial_rows() {
        let edge = CodeEdge::new("repo/a.rs::main", "repo/b.rs::parse");

        assert!(rank_hubs(&[], 4).is_empty());
        assert!(rank_hubs(&[edge], 0).is_empty());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R654.2
    #[test]
    fn depth_cap_and_hub_order() {
        let index = CodeIndex {
            symbols: ["a::main", "b::parse", "c::normalize", "d::emit", "e::write"]
                .map(String::from)
                .to_vec(),
            edges: vec![
                CodeEdge::new("a::main", "b::parse"),
                CodeEdge::new("b::parse", "c::normalize"),
                CodeEdge::new("c::normalize", "d::emit"),
                CodeEdge::new("d::emit", "e::write"),
            ],
            missing_repositories: Vec::new(),
        };
        let request = |depth| CodeContextRequest {
            anchor: "main".into(),
            depth,
            max_nodes: None,
        };
        let answer = code_context(&index, &request(Some(2))).unwrap();
        assert_eq!(answer.status, CodeContextStatus::Ok);
        assert_eq!(answer.anchor.as_deref(), Some("a::main"));
        let visited: Vec<_> = answer
            .nodes
            .iter()
            .map(|n| (n.symbol.as_str(), n.hops))
            .collect();
        assert_eq!(
            visited,
            vec![("a::main", 0), ("b::parse", 1), ("c::normalize", 2)]
        );
        assert_eq!(answer.hubs[0].symbol, "b::parse");
        assert_eq!(
            (answer.hubs[0].in_degree, answer.hubs[0].out_degree),
            (1, 1)
        );
        let default = code_context(&index, &request(None)).unwrap();
        assert_eq!(default.nodes.len(), 4);
        let hubs: Vec<_> = default.hubs.iter().map(|h| h.symbol.as_str()).collect();
        assert_eq!(&hubs[..2], &["b::parse", "c::normalize"]);
        assert_eq!(
            code_context(&index, &request(Some(7))).unwrap_err().code(),
            "CODE_DEPTH_EXCEEDED"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R654.7
    #[test]
    fn diagnose_orders_chain_by_stage() {
        let node = |id: &str, label: &str, at_ms| OpsNode {
            id: id.into(),
            label: label.into(),
            at_ms,
        };
        let link = |from: &str, to: &str, kind, ev: &str| OpsLink {
            from: from.into(),
            to: to.into(),
            kind,
            evidence_ids: vec![ev.into()],
        };
        let nodes = vec![
            node("anomaly", "HealthAnomaly", 50),
            node("svc", "Service", 10),
            node("host", "Host", 5),
            node("deploy", "Deployment", 40),
            node("chg", "Change", 30),
            node("cfg", "ConfigItem", 20),
        ];
        let links = vec![
            link("svc", "anomaly", OpsLinkKind::DependsOn, "e1"),
            link("host", "svc", OpsLinkKind::DependsOn, "e2"),
            link("deploy", "svc", OpsLinkKind::Change, "e3"),
            link("chg", "deploy", OpsLinkKind::Change, "e4"),
            link("cfg", "chg", OpsLinkKind::Change, "e5"),
        ];
        let diagnosis = diagnose(&nodes, &links, "anomaly").unwrap();
        let chain: Vec<_> = diagnosis
            .chain
            .iter()
            .map(|n| (n.id.as_str(), n.stage))
            .collect();
        assert_eq!(
            chain,
            vec![
                ("chg", OpsStage::Change),
                ("deploy", OpsStage::Deploy),
                ("svc", OpsStage::Dependency),
                ("host", OpsStage::Resource),
                ("anomaly", OpsStage::Symptom),
            ]
        );
        assert!(diagnosis.links.iter().all(|l| l.evidence_ids.len() == 1));
        assert!(diagnose(&nodes, &links, "svc").is_err());
    }
}
