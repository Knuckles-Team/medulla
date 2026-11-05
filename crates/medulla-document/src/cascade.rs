//! Document delete cascade planning (MEDULLA-UNIFIED-DATA-PLANE-R651.1/.2).
//!
//! The planner walks back-references from a Document and returns the sorted,
//! unique node and edge set to delete, with a SHA-256 digest over the canonical
//! plan bytes. It depends on no graph crate: callers implement [`CascadeReader`].

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable refusal code for a cascade above `document.max_cascade_nodes`.
pub const CASCADE_TOO_LARGE: &str = "CASCADE_TOO_LARGE";
/// Label of the cascade root.
pub const DOCUMENT_LABEL: &str = "Document";
/// In-edge labels the cascade follows toward the Document.
pub const CASCADE_EDGE_LABELS: [&str; 3] = ["chunkOf", "extractedFrom", "enrichmentOf"];

/// One graph node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeKey {
    pub label: String,
    pub id: String,
}

/// One directed graph edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeKey {
    pub from: NodeKey,
    pub label: String,
    pub to: NodeKey,
}

/// Sorted, unique nodes and edges to delete for one Document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "CascadePlanWire")]
pub struct CascadePlan {
    pub tenant: String,
    pub document_id: String,
    pub nodes: Vec<NodeKey>,
    pub edges: Vec<EdgeKey>,
    /// SHA-256 over [`CascadePlan::canonical_bytes`], 64 lowercase hex.
    pub plan_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CascadePlanWire {
    tenant: String,
    document_id: String,
    nodes: Vec<NodeKey>,
    edges: Vec<EdgeKey>,
    plan_digest: String,
}

impl TryFrom<CascadePlanWire> for CascadePlan {
    type Error = String;

    fn try_from(wire: CascadePlanWire) -> Result<Self, String> {
        if !strictly_sorted(&wire.nodes) || !strictly_sorted(&wire.edges) {
            return Err("cascade plan nodes and edges must be sorted and unique".into());
        }
        let plan = CascadePlan::new(wire.tenant, wire.document_id, wire.nodes, wire.edges);
        if plan.plan_digest != wire.plan_digest {
            return Err("cascade plan digest does not match its contents".into());
        }
        Ok(plan)
    }
}

fn strictly_sorted<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|pair| pair[0] < pair[1])
}

fn put(buf: &mut Vec<u8>, field: &str) {
    buf.extend_from_slice(&(field.len() as u64).to_be_bytes());
    buf.extend_from_slice(field.as_bytes());
}

fn put_node(buf: &mut Vec<u8>, node: &NodeKey) {
    put(buf, &node.label);
    put(buf, &node.id);
}

impl CascadePlan {
    /// Build a plan from already sorted, unique keys and compute its digest.
    fn new(tenant: String, document_id: String, nodes: Vec<NodeKey>, edges: Vec<EdgeKey>) -> Self {
        let mut plan = Self {
            tenant,
            document_id,
            nodes,
            edges,
            plan_digest: String::new(),
        };
        plan.plan_digest = format!("{:x}", Sha256::digest(plan.canonical_bytes()));
        plan
    }

    /// Length-prefixed bytes of every field but the digest, in field order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        put(&mut buf, &self.tenant);
        put(&mut buf, &self.document_id);
        buf.extend_from_slice(&(self.nodes.len() as u64).to_be_bytes());
        for node in &self.nodes {
            put_node(&mut buf, node);
        }
        buf.extend_from_slice(&(self.edges.len() as u64).to_be_bytes());
        for edge in &self.edges {
            put_node(&mut buf, &edge.from);
            put(&mut buf, &edge.label);
            put_node(&mut buf, &edge.to);
        }
        buf
    }
}

/// What a committed cascade left behind; all zeros once indexes drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrphanReport {
    pub graph_rows_left: u64,
    pub edges_left: u64,
    pub index_pending: u64,
}

/// Why a cascade could not be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascadeRefusal {
    /// More than `max_nodes` nodes would be deleted.
    TooLarge { max_nodes: usize },
}

impl CascadeRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => CASCADE_TOO_LARGE,
        }
    }
}

impl std::fmt::Display for CascadeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { max_nodes } => {
                write!(f, "{}: cascade exceeds {max_nodes} nodes", self.code())
            }
        }
    }
}

impl std::error::Error for CascadeRefusal {}

/// The RLS-visible edge view the planner reads.
pub trait CascadeReader {
    /// Edges whose `to` is `node`.
    fn in_edges(&self, node: &NodeKey) -> Vec<EdgeKey>;
    /// Edges whose `from` is `node`.
    fn out_edges(&self, node: &NodeKey) -> Vec<EdgeKey>;
}

/// Breadth-first plan over the cascade in-edges, starting at the Document.
///
/// O(planned nodes + edges). Every in- and out-edge of a planned node is
/// deleted so no dangling edge survives.
pub fn plan_cascade(
    reader: &dyn CascadeReader,
    tenant: &str,
    document_id: &str,
    max_nodes: usize,
) -> Result<CascadePlan, CascadeRefusal> {
    let root = NodeKey {
        label: DOCUMENT_LABEL.into(),
        id: document_id.into(),
    };
    let mut nodes = BTreeSet::from([root.clone()]);
    let mut edges = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    while let Some(node) = queue.pop_front() {
        for edge in reader.in_edges(&node) {
            if CASCADE_EDGE_LABELS.contains(&edge.label.as_str()) && nodes.insert(edge.from.clone())
            {
                if nodes.len() > max_nodes {
                    return Err(CascadeRefusal::TooLarge { max_nodes });
                }
                queue.push_back(edge.from.clone());
            }
            edges.insert(edge);
        }
        edges.extend(reader.out_edges(&node));
    }
    if nodes.len() > max_nodes {
        return Err(CascadeRefusal::TooLarge { max_nodes });
    }
    Ok(CascadePlan::new(
        tenant.into(),
        document_id.into(),
        nodes.into_iter().collect(),
        edges.into_iter().collect(),
    ))
}
