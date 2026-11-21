//! Accepting a changeset into the lifecycle graph (MEDULLA-SDLC-R001.3).
//!
//! Plan then apply. `medulla-source-types` plans the changeset node
//! (`plan_changeset_node`); this module derives the edges through
//! [`LifecycleStore::accept_changeset`] and returns both as one typed write set. The served
//! caller applies the write set as one mutation batch, the node ahead of its edges. The graph
//! is the store: the in-memory [`LifecycleStore`] is the pure model the derivation checks
//! endpoints against.

use medulla_source_types::lifecycle::{
    ChangesetNodeWrite, LifecycleEdgeKind, LifecycleKind, LifecycleRefusal,
};
use medulla_source_types::source_changeset::{commit_node_id, Changeset};

use super::derive::{derive_lifecycle_edges, CodeGraphView, DerivedEdge, Span};
use super::store::LifecycleStore;

/// What one accept writes: the changeset node, then its derived edges between graph node ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesetWrites {
    pub node: ChangesetNodeWrite,
    pub edges: Vec<DerivedEdge>,
    /// The trailer-derived edges dropped because the record they name is not in the graph
    /// yet: `(edge kind, the missing graph node id)`.
    pub unresolved: Vec<(LifecycleEdgeKind, String)>,
}

/// A trailer names a requirement or a defect that may not be ingested yet; the edge it
/// derives waits for the record instead of refusing the accept.
fn names_a_record(kind: LifecycleEdgeKind) -> bool {
    matches!(
        kind,
        LifecycleEdgeKind::CitesRequirement
            | LifecycleEdgeKind::Implements
            | LifecycleEdgeKind::FixedBy
    )
}

/// The graph node id of one derived-edge endpoint. A changeset is its `commit:` node; the
/// requirement end of `citesRequirement`, `implements` and `verifiedBy` is the requirement
/// record `requirement:<repo>:<row>`; every other endpoint already is a node id.
pub fn graph_endpoint(
    changeset: &Changeset,
    kind: LifecycleEdgeKind,
    from_side: bool,
    raw: &str,
) -> String {
    if raw == changeset.id {
        return commit_node_id(raw);
    }
    let requirement = match kind {
        LifecycleEdgeKind::CitesRequirement | LifecycleEdgeKind::Implements => !from_side,
        LifecycleEdgeKind::VerifiedBy => from_side,
        _ => false,
    };
    if requirement {
        format!("requirement:{}:{raw}", changeset.repo)
    } else {
        raw.to_owned()
    }
}

/// The byte span of the new file that differs from the old one: everything between the common
/// prefix and the common suffix. `None` when the bytes are equal; an insertion is an empty
/// span at its position.
pub fn changed_span(old: &[u8], new: &[u8]) -> Option<Span> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Some(Span {
        start: clamp(prefix),
        end: clamp(new.len() - suffix),
    })
}

/// Derive the edges of an accepted changeset and pair them with its node. `known` answers
/// what the graph holds under a node id (its lifecycle kind), or `None` when it holds nothing
/// the caller may see. A derived edge whose endpoint is unknown refuses the whole write with
/// `LIFECYCLE_ENDPOINT_UNKNOWN` and nothing is returned; only an edge a trailer derives
/// (`citesRequirement`, `implements`, `fixedBy`) is dropped and reported in `unresolved`.
pub fn accept_changeset_edges(
    store: &mut LifecycleStore,
    changeset: &Changeset,
    view: &CodeGraphView,
    node: ChangesetNodeWrite,
    known: &dyn Fn(&str) -> Option<LifecycleKind>,
) -> Result<ChangesetWrites, LifecycleRefusal> {
    let mut kept = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen = Vec::new();
    'edges: for edge in derive_lifecycle_edges(changeset, view) {
        let mut ends = Vec::new();
        for (from_side, raw) in [(true, &edge.from), (false, &edge.to)] {
            if *raw == changeset.id {
                continue;
            }
            let id = graph_endpoint(changeset, edge.kind, from_side, raw);
            match known(&id) {
                Some(kind) => ends.push((raw.clone(), kind)),
                None if names_a_record(edge.kind) => {
                    unresolved.push((edge.kind, id));
                    continue 'edges;
                }
                None => return Err(LifecycleRefusal::EndpointUnknown(id)),
            }
        }
        seen.extend(ends);
        kept.push(edge);
    }
    store.add_node(&changeset.id, LifecycleKind::Changeset);
    for (raw, kind) in seen {
        store.add_node(&raw, kind);
    }
    store.apply_derived(kept.clone())?;
    let edges = kept
        .into_iter()
        .map(|edge| DerivedEdge {
            from: graph_endpoint(changeset, edge.kind, true, &edge.from),
            to: graph_endpoint(changeset, edge.kind, false, &edge.to),
            ..edge
        })
        .collect();
    Ok(ChangesetWrites {
        node,
        edges,
        unresolved,
    })
}
