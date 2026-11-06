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
use serde::{Deserialize, Serialize};

/// Which code-context question a request asks (MEDULLA-FEDERATED-QUERY-R654.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeQuestion {
    How,
    Usage,
    Impact,
}

/// A served `CodeContext` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeContextRequest {
    /// Qualified symbol name, or a suffix of one.
    pub anchor: String,
    pub question: CodeQuestion,
    /// Walk depth in hops; `None` takes `knob code.context_depth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Visited-node cap; `None` takes `knob code.max_nodes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Outcome of a code-context walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeContextStatus {
    Ok,
    Ambiguous,
    Degraded,
}

/// One symbol reached by the walk with its hop count from the anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeNode {
    pub symbol: String,
    pub hops: u32,
}

/// A hub with its centrality inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedHub {
    pub symbol: String,
    pub in_degree: u32,
    pub out_degree: u32,
    pub score: f64,
}

/// The served `CodeContext` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeContextAnswer {
    pub status: CodeContextStatus,
    /// The resolved anchor; empty when `status` is `ambiguous`.
    pub anchors: Vec<String>,
    /// Symbols that reach the anchor (callers), by hop count.
    pub usages: Vec<CodeNode>,
    /// Symbols the anchor reaches (callees), by hop count.
    pub impact: Vec<CodeNode>,
    pub hubs: Vec<RankedHub>,
    /// Candidate anchors when `status` is `ambiguous`.
    pub candidates: Vec<String>,
}

/// The closed set of operations causal stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpsCausalStage {
    Change,
    Deploy,
    Config,
    Dependency,
    Resource,
    Symptom,
}

/// One link of a diagnosed causal chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalLink {
    pub stage: OpsCausalStage,
    pub node_id: String,
    pub relationship: String,
    pub evidence_ids: Vec<String>,
}

/// The served `Diagnose` answer: a symptom and its causal chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymptomDiagnosis {
    pub symptom: String,
    pub chain: Vec<CausalLink>,
}

/// A directed caller-to-callee edge in a resolved code subgraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        .limit
        .map_or(DEFAULT_MAX_NODES, |limit| limit as usize)
        .clamp(MIN_MAX_NODES, MAX_MAX_NODES);
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
                anchors: Vec::new(),
                usages: Vec::new(),
                impact: Vec::new(),
                hubs: Vec::new(),
                candidates,
            })
        }
    };

    let mut forward: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut backward: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in &index.edges {
        forward
            .entry(edge.caller.as_str())
            .or_default()
            .insert(edge.callee.as_str());
        backward
            .entry(edge.callee.as_str())
            .or_default()
            .insert(edge.caller.as_str());
    }
    let walk = |adjacency: &BTreeMap<&str, BTreeSet<&str>>| -> BTreeMap<String, u32> {
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
        hops
    };
    let want_usages = matches!(request.question, CodeQuestion::How | CodeQuestion::Usage);
    let want_impact = matches!(request.question, CodeQuestion::How | CodeQuestion::Impact);
    let callers = if want_usages {
        walk(&backward)
    } else {
        BTreeMap::new()
    };
    let callees = if want_impact {
        walk(&forward)
    } else {
        BTreeMap::new()
    };

    let visited: BTreeSet<&str> = callers
        .keys()
        .chain(callees.keys())
        .map(String::as_str)
        .collect();
    let mut edges: Vec<CodeEdge> = index
        .edges
        .iter()
        .filter(|edge| {
            visited.contains(edge.caller.as_str()) && visited.contains(edge.callee.as_str())
        })
        .cloned()
        .collect();
    edges.sort_by(|a, b| (&a.caller, &a.callee).cmp(&(&b.caller, &b.callee)));
    edges.dedup();
    let mut in_degree: BTreeMap<&str, u32> = BTreeMap::new();
    let mut out_degree: BTreeMap<&str, u32> = BTreeMap::new();
    for edge in &edges {
        *out_degree.entry(edge.caller.as_str()).or_default() += 1;
        *in_degree.entry(edge.callee.as_str()).or_default() += 1;
    }
    let hubs = rank_hubs(&edges, visited.len())
        .into_iter()
        .map(|hub| RankedHub {
            in_degree: in_degree.get(hub.symbol.as_str()).copied().unwrap_or(0),
            out_degree: out_degree.get(hub.symbol.as_str()).copied().unwrap_or(0),
            score: hub.centrality,
            symbol: hub.symbol,
        })
        .collect();
    let to_nodes = |hops: BTreeMap<String, u32>| -> Vec<CodeNode> {
        let mut nodes: Vec<CodeNode> = hops
            .into_iter()
            .filter(|(symbol, _)| *symbol != anchor)
            .map(|(symbol, hops)| CodeNode { symbol, hops })
            .collect();
        nodes.sort_by(|a, b| a.hops.cmp(&b.hops).then_with(|| a.symbol.cmp(&b.symbol)));
        nodes
    };
    Ok(CodeContextAnswer {
        status: if index.missing_repositories.is_empty() {
            CodeContextStatus::Ok
        } else {
            CodeContextStatus::Degraded
        },
        anchors: vec![anchor.clone()],
        usages: to_nodes(callers),
        impact: to_nodes(callees),
        hubs,
        candidates: Vec::new(),
    })
}

/// Stage by ontology label (`infrastructure-v1.ttl`).
pub const OPS_STAGE_BY_LABEL: &[(&str, OpsCausalStage)] = &[
    ("Change", OpsCausalStage::Change),
    ("Deployment", OpsCausalStage::Deploy),
    ("ConfigItem", OpsCausalStage::Config),
    ("Service", OpsCausalStage::Dependency),
    ("Host", OpsCausalStage::Resource),
    ("HealthAnomaly", OpsCausalStage::Symptom),
];

fn stage_of(label: &str) -> Option<OpsCausalStage> {
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
        .filter(|node| stage_of(&node.label) == Some(OpsCausalStage::Symptom))
    else {
        return Err(CodeContextError::AnchorUnknown {
            anchor: symptom.to_string(),
        });
    };
    // node id -> (hops, relationship and evidence of the link that reached it)
    let mut reached: BTreeMap<&str, (u32, &'static str, Vec<String>)> =
        BTreeMap::from([(start.id.as_str(), (0, "symptom", Vec::new()))]);
    let mut queue = VecDeque::from([(start.id.as_str(), 0u32)]);
    while let Some((id, hop)) = queue.pop_front() {
        if hop >= DIAGNOSE_DEPTH {
            continue;
        }
        for link in links.iter().filter(|link| link.to == id) {
            let Some(cause) = by_id.get(link.from.as_str()) else {
                continue;
            };
            if stage_of(&cause.label).is_none() || reached.contains_key(cause.id.as_str()) {
                continue;
            }
            let relationship = match link.kind {
                OpsLinkKind::DependsOn => "depends_on",
                OpsLinkKind::Change => "change",
            };
            reached.insert(
                cause.id.as_str(),
                (hop + 1, relationship, link.evidence_ids.clone()),
            );
            queue.push_back((cause.id.as_str(), hop + 1));
        }
    }
    let mut ordered: Vec<(OpsCausalStage, i64, CausalLink)> = reached
        .iter()
        .filter_map(|(id, (_, relationship, evidence_ids))| {
            let node = by_id.get(id)?;
            let stage = stage_of(&node.label)?;
            Some((
                stage,
                node.at_ms,
                CausalLink {
                    stage,
                    node_id: node.id.clone(),
                    relationship: (*relationship).to_string(),
                    evidence_ids: evidence_ids.clone(),
                },
            ))
        })
        .collect();
    ordered.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.node_id.cmp(&b.2.node_id))
    });
    Ok(SymptomDiagnosis {
        symptom: symptom.to_string(),
        chain: ordered.into_iter().map(|(_, _, link)| link).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R654.1
    #[test]
    fn types_round_trip_and_reject_unknown_stage() {
        let request = CodeContextRequest {
            anchor: "repo/b.rs::parse".to_string(),
            question: CodeQuestion::Impact,
            depth: Some(3),
            limit: None,
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(
            text,
            r#"{"anchor":"repo/b.rs::parse","question":"impact","depth":3}"#
        );
        assert_eq!(
            serde_json::from_str::<CodeContextRequest>(&text).unwrap(),
            request
        );
        assert!(
            serde_json::from_str::<CodeContextRequest>(r#"{"anchor":"a","question":"why"}"#)
                .is_err()
        );

        let answer = CodeContextAnswer {
            status: CodeContextStatus::Ambiguous,
            anchors: vec![],
            usages: vec![CodeNode {
                symbol: "repo/a.rs::main".to_string(),
                hops: 1,
            }],
            impact: vec![],
            hubs: vec![RankedHub {
                symbol: "repo/b.rs::parse".to_string(),
                in_degree: 1,
                out_degree: 2,
                score: 0.25,
            }],
            candidates: vec!["x::parse".to_string(), "y::parse".to_string()],
        };
        let text = serde_json::to_string(&answer).unwrap();
        assert!(text.contains(r#""status":"ambiguous""#));
        assert_eq!(
            serde_json::from_str::<CodeContextAnswer>(&text).unwrap(),
            answer
        );

        let stages = [
            OpsCausalStage::Change,
            OpsCausalStage::Deploy,
            OpsCausalStage::Config,
            OpsCausalStage::Dependency,
            OpsCausalStage::Resource,
            OpsCausalStage::Symptom,
        ];
        let names: Vec<String> = stages
            .iter()
            .map(|stage| serde_json::to_string(stage).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                r#""change""#,
                r#""deploy""#,
                r#""config""#,
                r#""dependency""#,
                r#""resource""#,
                r#""symptom""#
            ]
        );
        let diagnosis = SymptomDiagnosis {
            symptom: "latency".to_string(),
            chain: vec![CausalLink {
                stage: OpsCausalStage::Deploy,
                node_id: "deploy-7".to_string(),
                relationship: "CAUSED".to_string(),
                evidence_ids: vec!["ev-1".to_string()],
            }],
        };
        let text = serde_json::to_string(&diagnosis).unwrap();
        assert_eq!(
            serde_json::from_str::<SymptomDiagnosis>(&text).unwrap(),
            diagnosis
        );
        assert!(serde_json::from_str::<OpsCausalStage>(r#""weather""#).is_err());
        assert!(
            serde_json::from_str::<SymptomDiagnosis>(&text.replace("deploy\"", "weather\""))
                .is_err()
        );
    }

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
            question: CodeQuestion::Impact,
            depth,
            limit: None,
        };
        let answer = code_context(&index, &request(Some(2))).unwrap();
        assert_eq!(answer.status, CodeContextStatus::Ok);
        assert_eq!(answer.anchors, vec!["a::main".to_string()]);
        assert!(answer.usages.is_empty());
        let visited: Vec<_> = answer
            .impact
            .iter()
            .map(|n| (n.symbol.as_str(), n.hops))
            .collect();
        assert_eq!(visited, vec![("b::parse", 1), ("c::normalize", 2)]);
        assert_eq!(answer.hubs[0].symbol, "b::parse");
        assert_eq!(
            (answer.hubs[0].in_degree, answer.hubs[0].out_degree),
            (1, 1)
        );
        let default = code_context(&index, &request(None)).unwrap();
        assert_eq!(default.impact.len(), 3);
        let hubs: Vec<_> = default.hubs.iter().map(|h| h.symbol.as_str()).collect();
        assert_eq!(&hubs[..2], &["b::parse", "c::normalize"]);
        assert_eq!(
            code_context(&index, &request(Some(7))).unwrap_err().code(),
            "CODE_DEPTH_EXCEEDED"
        );
    }

    /// Verifies MEDULLA-FEDERATED-QUERY-R654.7: Behavior: symptom diagnosis
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
            .map(|n| (n.node_id.as_str(), n.stage))
            .collect();
        assert_eq!(
            chain,
            vec![
                ("chg", OpsCausalStage::Change),
                ("deploy", OpsCausalStage::Deploy),
                ("svc", OpsCausalStage::Dependency),
                ("host", OpsCausalStage::Resource),
                ("anomaly", OpsCausalStage::Symptom),
            ]
        );
        assert!(diagnosis
            .chain
            .iter()
            .filter(|l| l.stage != OpsCausalStage::Symptom)
            .all(|l| l.evidence_ids.len() == 1));
        assert!(diagnose(&nodes, &links, "svc").is_err());
    }
}
