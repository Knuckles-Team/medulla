//! Recursive trace node and its snapshot checks (MEDULLA-DECISION-ENGINE-R133.7.1).
//!
//! A recursive reasoning run stores its code runs, sub-calls and intermediates
//! as typed nodes. One snapshot is the set of nodes a terminal commit stores;
//! [`validate_snapshot`] refuses an incomplete ancestry, a cycle or a malformed
//! digest before anything is written. Pure data and checks; the commit path
//! lives in `medulla-work-store`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// The most children one node names.
pub const MAX_CHILD_IDS: usize = 64;
/// The most evidence references one node carries.
pub const MAX_EVIDENCE_REFS: usize = 32;
/// The most input or output references one node carries.
pub const MAX_IO_REFS: usize = 32;

/// A reference with the SHA-256 of what it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DigestRef {
    pub ref_id: String,
    /// 64-character lowercase hexadecimal SHA-256.
    pub digest: String,
}

/// What a recursive trace node records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TraceNodeKind {
    /// A code run the reasoning executed.
    CodeRun,
    /// A sub-call the reasoning dispatched.
    SubCall,
    /// An intermediate result.
    Intermediate,
}

impl TraceNodeKind {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodeRun => "code_run",
            Self::SubCall => "sub_call",
            Self::Intermediate => "intermediate",
        }
    }

    /// Parse a wire spelling.
    pub fn parse(value: &str) -> Option<Self> {
        [Self::CodeRun, Self::SubCall, Self::Intermediate]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

/// One node of a recursive reasoning trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RecursiveTraceNode {
    pub schema_version: u16,
    pub node_id: String,
    pub run_id: String,
    pub run_revision: u64,
    pub step_id: String,
    pub step_revision: u64,
    pub kind: TraceNodeKind,
    pub parent_node_id: Option<String>,
    pub child_ids: BoundedVec<String, MAX_CHILD_IDS>,
    pub input_refs: BoundedVec<DigestRef, MAX_IO_REFS>,
    pub output_refs: BoundedVec<DigestRef, MAX_IO_REFS>,
    pub evidence_refs: BoundedVec<DigestRef, MAX_EVIDENCE_REFS>,
    pub source_revision: u64,
    pub trace_snapshot_id: String,
}

/// Why a snapshot is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceRefusal {
    /// A parent or child is missing, a link is not mutual, or a node belongs to
    /// another snapshot.
    IncompleteAncestry { node_id: String },
    /// The parent links form a cycle.
    Cycle { node_id: String },
    /// A digest is not 64 lowercase hexadecimal characters.
    MissingDigest { node_id: String },
}

impl TraceRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::IncompleteAncestry { .. } => "TRACE_INCOMPLETE_ANCESTRY",
            Self::Cycle { .. } => "TRACE_CYCLE",
            Self::MissingDigest { .. } => "TRACE_MISSING_DIGEST",
        }
    }

    /// The node the refusal names.
    pub fn node_id(&self) -> &str {
        match self {
            Self::IncompleteAncestry { node_id }
            | Self::Cycle { node_id }
            | Self::MissingDigest { node_id } => node_id,
        }
    }
}

impl std::fmt::Display for TraceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: node '{}'", self.code(), self.node_id())
    }
}

fn digest_valid(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Check one snapshot: every digest is well formed, all nodes share one
/// `trace_snapshot_id`, every parent and child exists with a mutual link, and
/// the parent links have no cycle.
pub fn validate_snapshot(nodes: &[RecursiveTraceNode]) -> Result<(), TraceRefusal> {
    let by_id: BTreeMap<&str, &RecursiveTraceNode> =
        nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
    for node in nodes {
        let refs = node
            .input_refs
            .iter()
            .chain(node.output_refs.iter())
            .chain(node.evidence_refs.iter());
        for reference in refs {
            if !digest_valid(&reference.digest) {
                return Err(TraceRefusal::MissingDigest {
                    node_id: node.node_id.clone(),
                });
            }
        }
    }
    let snapshot = nodes.first().map(|n| n.trace_snapshot_id.as_str());
    for node in nodes {
        let incomplete = || TraceRefusal::IncompleteAncestry {
            node_id: node.node_id.clone(),
        };
        if Some(node.trace_snapshot_id.as_str()) != snapshot {
            return Err(incomplete());
        }
        if let Some(parent) = &node.parent_node_id {
            let found = by_id.get(parent.as_str()).ok_or_else(incomplete)?;
            if !found.child_ids.iter().any(|c| c == &node.node_id) {
                return Err(incomplete());
            }
        }
        for child in node.child_ids.iter() {
            let found = by_id.get(child.as_str()).ok_or_else(incomplete)?;
            if found.parent_node_id.as_deref() != Some(node.node_id.as_str()) {
                return Err(incomplete());
            }
        }
    }
    for node in nodes {
        let mut seen = BTreeSet::new();
        let mut cursor = node;
        while let Some(parent) = cursor.parent_node_id.as_deref() {
            if !seen.insert(cursor.node_id.as_str()) {
                return Err(TraceRefusal::Cycle {
                    node_id: node.node_id.clone(),
                });
            }
            cursor = by_id[parent];
        }
    }
    Ok(())
}

/// Fields of a node that name stored content; reading them needs `trace:read-content`.
pub const PROJECTION_CONTENT: &[&str] = &["input_refs", "output_refs", "evidence_refs"];
/// Structural fields every node view carries; naming one in a projection is allowed.
pub const PROJECTION_STRUCTURE: &[&str] = &[
    "node_id",
    "run_id",
    "run_revision",
    "step_id",
    "step_revision",
    "kind",
    "parent_node_id",
    "child_ids",
    "source_revision",
];

/// One authorized read of a stored recursive trace (MEDULLA-DECISION-ENGINE-R133.7.3).
///
/// The page is the pre-order walk of the subtree under `root_node_id`, keeping
/// the nodes whose kind is in `kinds` (all kinds when empty). `snapshot_id` and
/// `snapshot_revision` must name the root's stored snapshot and source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceCritiqueQuery {
    /// The tenant that owns the trace; it must equal the verified carrier tenant.
    pub tenant: String,
    pub root_node_id: String,
    pub snapshot_id: String,
    pub snapshot_revision: u64,
    /// Extra fields to return per selected node. Content fields need `trace:read-content`.
    #[serde(default)]
    pub projection: Vec<String>,
    /// Keep only nodes of these kinds (`code_run`, `sub_call`, `intermediate`).
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    pub limit: u32,
}

/// One node of a critique page. Content fields are present only when the
/// projection named them and the caller holds `trace:read-content`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceCritiqueNode {
    pub node_id: String,
    pub run_id: String,
    pub run_revision: u64,
    pub step_id: String,
    pub step_revision: u64,
    pub kind: TraceNodeKind,
    pub parent_node_id: Option<String>,
    pub child_ids: Vec<String>,
    pub source_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_refs: Option<Vec<DigestRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_refs: Option<Vec<DigestRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_refs: Option<Vec<DigestRef>>,
}

/// The source revision one selected node was read at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceSourceRef {
    pub node_id: String,
    pub source_revision: u64,
}

/// One page of a critique read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceCritiquePage {
    pub snapshot_id: String,
    pub snapshot_revision: u64,
    pub root_node_id: String,
    /// The selected nodes in pre-order.
    pub nodes: Vec<TraceCritiqueNode>,
    /// Every ancestor of a selected node up to the tree root that is not itself
    /// selected, in node-id order, structural fields only.
    pub ancestry: Vec<TraceCritiqueNode>,
    /// One entry per selected node.
    pub source_refs: Vec<TraceSourceRef>,
    /// Opaque; present when more nodes follow.
    pub next_cursor: Option<String>,
}

/// Why a critique read is refused. A refusal returns no node and no protected field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceReadRefusal {
    /// The projection names a content field the caller may not read, or a field that does not exist.
    ProjectionUnauthorized { field: String },
    /// The cursor does not belong to this root, snapshot, revision and kind filter.
    StaleCursor,
    /// The query's snapshot id or revision is not the root's stored one.
    SnapshotMismatch { expected: String, actual: String },
    /// The stored tree is incomplete, cyclic or malformed.
    Trace(TraceRefusal),
    /// The query itself is malformed (`INVALID_ARGUMENT`).
    InvalidQuery(String),
}

impl TraceReadRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProjectionUnauthorized { .. } => "TRACE_PROJECTION_UNAUTHORIZED",
            Self::StaleCursor => "TRACE_STALE_CURSOR",
            Self::SnapshotMismatch { .. } => "TRACE_SNAPSHOT_MISMATCH",
            Self::Trace(refusal) => refusal.code(),
            Self::InvalidQuery(_) => "INVALID_ARGUMENT",
        }
    }
}

impl std::fmt::Display for TraceReadRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProjectionUnauthorized { field } => {
                write!(f, "{}: field '{field}'", self.code())
            }
            Self::StaleCursor => write!(f, "{}: cursor does not match this read", self.code()),
            Self::SnapshotMismatch { expected, actual } => write!(
                f,
                "{}: query names '{expected}' but the stored snapshot is '{actual}'",
                self.code()
            ),
            Self::Trace(refusal) => write!(f, "{}: {}", refusal.code(), refusal),
            Self::InvalidQuery(detail) => write!(f, "{}: {detail}", self.code()),
        }
    }
}

/// A critique read that did not produce a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceReadError {
    Refused(TraceReadRefusal),
    /// The node lookup failed (storage error), passed through unchanged.
    Lookup(String),
}

impl TraceReadError {
    /// The wire error text: `CODE: detail` for a refusal, the storage text otherwise.
    pub fn into_wire(self) -> String {
        match self {
            Self::Refused(refusal) => refusal.to_string(),
            Self::Lookup(text) => text,
        }
    }
}

fn hex_encode(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Option<String> {
    if hex.len() % 2 != 0 || !hex.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

const CURSOR_SEPARATOR: char = '\u{1f}';

fn kind_names(kinds: &[TraceNodeKind]) -> String {
    let names: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
    names.join(",")
}

/// The opaque cursor for the page that starts after `offset` selected nodes.
fn encode_cursor(query: &TraceCritiqueQuery, kinds: &[TraceNodeKind], offset: usize) -> String {
    let parts = [
        query.snapshot_id.clone(),
        query.snapshot_revision.to_string(),
        query.root_node_id.clone(),
        kind_names(kinds),
        offset.to_string(),
    ];
    hex_encode(&parts.join(&CURSOR_SEPARATOR.to_string()))
}

/// The offset a cursor carries, `None` when it belongs to another read.
fn decode_cursor(
    query: &TraceCritiqueQuery,
    kinds: &[TraceNodeKind],
    cursor: &str,
) -> Option<usize> {
    let text = hex_decode(cursor)?;
    let parts: Vec<&str> = text.split(CURSOR_SEPARATOR).collect();
    let [snapshot, revision, root, cursor_kinds, offset] = parts.as_slice() else {
        return None;
    };
    (*snapshot == query.snapshot_id
        && *revision == query.snapshot_revision.to_string()
        && *root == query.root_node_id
        && *cursor_kinds == kind_names(kinds))
    .then(|| offset.parse().ok())
    .flatten()
}

fn clean(field: &str, value: &str) -> Result<(), TraceReadRefusal> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(TraceReadRefusal::InvalidQuery(format!(
            "{field} must be non-empty text without control characters"
        )));
    }
    Ok(())
}

/// Check the query alone: bounds, kinds and projection authority. Returns the
/// parsed, sorted and de-duplicated kind filter.
fn check_query(
    query: &TraceCritiqueQuery,
    content_authorized: bool,
    page_limit: u32,
) -> Result<Vec<TraceNodeKind>, TraceReadRefusal> {
    clean("tenant", &query.tenant)?;
    clean("root_node_id", &query.root_node_id)?;
    clean("snapshot_id", &query.snapshot_id)?;
    if query.limit == 0 || query.limit > page_limit {
        return Err(TraceReadRefusal::InvalidQuery(format!(
            "limit must be in [1, {page_limit}]"
        )));
    }
    for field in &query.projection {
        let content = PROJECTION_CONTENT.contains(&field.as_str());
        let structure = PROJECTION_STRUCTURE.contains(&field.as_str());
        if (content && !content_authorized) || !(content || structure) {
            return Err(TraceReadRefusal::ProjectionUnauthorized {
                field: field.clone(),
            });
        }
    }
    let mut kinds = Vec::new();
    for kind in &query.kinds {
        let parsed = TraceNodeKind::parse(kind).ok_or_else(|| {
            TraceReadRefusal::InvalidQuery(format!("unknown trace node kind '{kind}'"))
        })?;
        kinds.push(parsed);
    }
    kinds.sort();
    kinds.dedup();
    Ok(kinds)
}

fn view(node: &RecursiveTraceNode, projection: &[String]) -> TraceCritiqueNode {
    let wants = |field: &str| projection.iter().any(|p| p == field);
    TraceCritiqueNode {
        node_id: node.node_id.clone(),
        run_id: node.run_id.clone(),
        run_revision: node.run_revision,
        step_id: node.step_id.clone(),
        step_revision: node.step_revision,
        kind: node.kind,
        parent_node_id: node.parent_node_id.clone(),
        child_ids: node.child_ids.as_slice().to_vec(),
        source_revision: node.source_revision,
        input_refs: wants("input_refs").then(|| node.input_refs.as_slice().to_vec()),
        output_refs: wants("output_refs").then(|| node.output_refs.as_slice().to_vec()),
        evidence_refs: wants("evidence_refs").then(|| node.evidence_refs.as_slice().to_vec()),
    }
}

fn incomplete(node_id: &str) -> TraceReadError {
    TraceReadError::Refused(TraceReadRefusal::Trace(TraceRefusal::IncompleteAncestry {
        node_id: node_id.to_string(),
    }))
}

fn cycle(node_id: &str) -> TraceReadError {
    TraceReadError::Refused(TraceReadRefusal::Trace(TraceRefusal::Cycle {
        node_id: node_id.to_string(),
    }))
}

/// Read one critique page. `lookup` returns the stored node of `node_id` for
/// the query's tenant, or `None` when it is absent or another tenant's (the two
/// are indistinguishable, so a foreign root reads exactly like a missing one).
pub fn read_critique<F>(
    query: &TraceCritiqueQuery,
    content_authorized: bool,
    page_limit: u32,
    mut lookup: F,
) -> Result<TraceCritiquePage, TraceReadError>
where
    F: FnMut(&str) -> Result<Option<RecursiveTraceNode>, String>,
{
    let kinds =
        check_query(query, content_authorized, page_limit).map_err(TraceReadError::Refused)?;
    let root = lookup(&query.root_node_id)
        .map_err(TraceReadError::Lookup)?
        .ok_or_else(|| incomplete(&query.root_node_id))?;
    if root.trace_snapshot_id != query.snapshot_id
        || root.source_revision != query.snapshot_revision
    {
        return Err(TraceReadError::Refused(
            TraceReadRefusal::SnapshotMismatch {
                expected: format!("{}@{}", query.snapshot_id, query.snapshot_revision),
                actual: format!("{}@{}", root.trace_snapshot_id, root.source_revision),
            },
        ));
    }
    let offset = match &query.cursor {
        None => 0,
        Some(cursor) => decode_cursor(query, &kinds, cursor)
            .ok_or(TraceReadError::Refused(TraceReadRefusal::StaleCursor))?,
    };
    let mut fetch = |node_id: &str| -> Result<RecursiveTraceNode, TraceReadError> {
        let node = lookup(node_id)
            .map_err(TraceReadError::Lookup)?
            .ok_or_else(|| incomplete(node_id))?;
        if node.trace_snapshot_id != query.snapshot_id {
            return Err(incomplete(node_id));
        }
        Ok(node)
    };

    let mut selected: Vec<RecursiveTraceNode> = Vec::new();
    let mut seen_selected = 0usize;
    let mut more = false;
    let mut visited = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if !visited.insert(node.node_id.clone()) {
            return Err(cycle(&node.node_id));
        }
        if kinds.is_empty() || kinds.contains(&node.kind) {
            seen_selected += 1;
            if seen_selected > offset {
                if selected.len() >= query.limit as usize {
                    more = true;
                    break;
                }
                selected.push(node.clone());
            }
        }
        for child_id in node.child_ids.iter().rev() {
            let child = fetch(child_id)?;
            if child.parent_node_id.as_deref() != Some(node.node_id.as_str()) {
                return Err(incomplete(child_id));
            }
            stack.push(child);
        }
    }

    let selected_ids: BTreeSet<&str> = selected.iter().map(|n| n.node_id.as_str()).collect();
    let mut ancestors: BTreeMap<String, RecursiveTraceNode> = BTreeMap::new();
    for node in &selected {
        let mut chain = BTreeSet::from([node.node_id.clone()]);
        let mut parent = node.parent_node_id.clone();
        while let Some(parent_id) = parent {
            if !chain.insert(parent_id.clone()) {
                return Err(cycle(&parent_id));
            }
            if ancestors.contains_key(&parent_id) {
                break;
            }
            let found = fetch(&parent_id)?;
            parent = found.parent_node_id.clone();
            ancestors.insert(parent_id, found);
        }
    }
    let ancestry = ancestors
        .values()
        .filter(|node| !selected_ids.contains(node.node_id.as_str()))
        .map(|node| view(node, &[]))
        .collect();

    Ok(TraceCritiquePage {
        snapshot_id: query.snapshot_id.clone(),
        snapshot_revision: query.snapshot_revision,
        root_node_id: query.root_node_id.clone(),
        source_refs: selected
            .iter()
            .map(|node| TraceSourceRef {
                node_id: node.node_id.clone(),
                source_revision: node.source_revision,
            })
            .collect(),
        nodes: selected
            .iter()
            .map(|node| view(node, &query.projection))
            .collect(),
        ancestry,
        next_cursor: more.then(|| encode_cursor(query, &kinds, offset + selected.len())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, children: &[&str]) -> RecursiveTraceNode {
        RecursiveTraceNode {
            schema_version: 1,
            node_id: id.into(),
            run_id: "run:1".into(),
            run_revision: 1,
            step_id: format!("step:{id}"),
            step_revision: 1,
            kind: TraceNodeKind::CodeRun,
            parent_node_id: parent.map(str::to_string),
            child_ids: BoundedVec::new(children.iter().map(|c| c.to_string()).collect()).unwrap(),
            input_refs: BoundedVec::default(),
            output_refs: BoundedVec::default(),
            evidence_refs: BoundedVec::new(vec![DigestRef {
                ref_id: format!("evidence:{id}"),
                digest: "a".repeat(64),
            }])
            .unwrap(),
            source_revision: 1,
            trace_snapshot_id: "snapshot:1".into(),
        }
    }

    fn tree() -> Vec<RecursiveTraceNode> {
        vec![
            node("root", None, &["a", "b"]),
            node("a", Some("root"), &["c"]),
            node("b", Some("root"), &[]),
            node("c", Some("a"), &[]),
        ]
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.7.1
    #[test]
    fn snapshot_checks() {
        // A valid tree.
        assert_eq!(validate_snapshot(&tree()), Ok(()));

        // A missing parent.
        let mut missing_parent = tree();
        missing_parent.retain(|n| n.node_id != "a");
        let refusal = validate_snapshot(&missing_parent).unwrap_err();
        assert_eq!(refusal.code(), "TRACE_INCOMPLETE_ANCESTRY");

        // A child that does not name this node as parent.
        let mut wrong_parent = tree();
        wrong_parent[3].parent_node_id = Some("b".into());
        assert_eq!(
            validate_snapshot(&wrong_parent).unwrap_err().code(),
            "TRACE_INCOMPLETE_ANCESTRY"
        );

        // A node from another snapshot.
        let mut other_snapshot = tree();
        other_snapshot[2].trace_snapshot_id = "snapshot:2".into();
        assert_eq!(
            validate_snapshot(&other_snapshot).unwrap_err().code(),
            "TRACE_INCOMPLETE_ANCESTRY"
        );

        // A cycle: x and y name each other as parent and child.
        let cycle = vec![node("x", Some("y"), &["y"]), node("y", Some("x"), &["x"])];
        let refusal = validate_snapshot(&cycle).unwrap_err();
        assert_eq!(refusal.code(), "TRACE_CYCLE");

        // A short digest.
        let mut short = tree();
        short[1].evidence_refs = BoundedVec::new(vec![DigestRef {
            ref_id: "evidence:a".into(),
            digest: "abc".into(),
        }])
        .unwrap();
        let refusal = validate_snapshot(&short).unwrap_err();
        assert_eq!(refusal.code(), "TRACE_MISSING_DIGEST");
        assert_eq!(refusal.node_id(), "a");
    }

    fn query(root: &str) -> TraceCritiqueQuery {
        TraceCritiqueQuery {
            tenant: "tenant-a".into(),
            root_node_id: root.into(),
            snapshot_id: "snapshot:1".into(),
            snapshot_revision: 1,
            projection: Vec::new(),
            kinds: Vec::new(),
            cursor: None,
            limit: 2,
        }
    }

    fn read(
        query: &TraceCritiqueQuery,
        content: bool,
    ) -> Result<TraceCritiquePage, TraceReadError> {
        let stored = tree();
        read_critique(query, content, 8, |id| {
            Ok(stored.iter().find(|n| n.node_id == id).cloned())
        })
    }

    fn code(error: TraceReadError) -> &'static str {
        match error {
            TraceReadError::Refused(refusal) => refusal.code(),
            TraceReadError::Lookup(_) => "LOOKUP",
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.7.3
    #[test]
    fn critique_pages_in_preorder_with_ancestry() {
        let first = read(&query("a"), false).unwrap();
        let ids: Vec<_> = first.nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        let ancestry: Vec<_> = first.ancestry.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ancestry, ["root"]);
        assert!(first.next_cursor.is_none());

        let mut whole = query("root");
        let page = read(&whole, false).unwrap();
        let ids: Vec<_> = page.nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, ["root", "a"]);
        assert!(page.ancestry.is_empty());
        whole.cursor = page.next_cursor;
        let page = read(&whole, false).unwrap();
        let ids: Vec<_> = page.nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
        assert_eq!(page.ancestry.len(), 2);
        assert!(page.next_cursor.is_none());
        assert_eq!(page.source_refs.len(), 2);
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.7.3
    #[test]
    fn critique_content_needs_authority_and_refusals_carry_nothing() {
        let mut asked = query("root");
        asked.projection = vec!["evidence_refs".into()];
        assert_eq!(
            code(read(&asked, false).unwrap_err()),
            "TRACE_PROJECTION_UNAUTHORIZED"
        );
        let page = read(&asked, true).unwrap();
        assert!(page.nodes.iter().all(|n| n.evidence_refs.is_some()));
        assert!(page.nodes.iter().all(|n| n.input_refs.is_none()));
        assert!(page.ancestry.iter().all(|n| n.evidence_refs.is_none()));

        let mut stale = query("root");
        stale.cursor = Some("zz".into());
        assert_eq!(code(read(&stale, false).unwrap_err()), "TRACE_STALE_CURSOR");
        let mut moved = query("root");
        moved.cursor = read(&query("root"), false).unwrap().next_cursor;
        moved.snapshot_revision = 1;
        moved.root_node_id = "a".into();
        assert_eq!(code(read(&moved, false).unwrap_err()), "TRACE_STALE_CURSOR");

        let mut wrong = query("root");
        wrong.snapshot_id = "snapshot:2".into();
        assert_eq!(
            code(read(&wrong, false).unwrap_err()),
            "TRACE_SNAPSHOT_MISMATCH"
        );
        let mut revision = query("root");
        revision.snapshot_revision = 9;
        assert_eq!(
            code(read(&revision, false).unwrap_err()),
            "TRACE_SNAPSHOT_MISMATCH"
        );
        assert_eq!(
            code(read(&query("missing"), false).unwrap_err()),
            "TRACE_INCOMPLETE_ANCESTRY"
        );
        let mut kind = query("root");
        kind.kinds = vec!["nonsense".into()];
        assert_eq!(code(read(&kind, false).unwrap_err()), "INVALID_ARGUMENT");
    }
}
