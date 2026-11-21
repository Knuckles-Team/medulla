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
    /// The trailer-derived edges whose record is not in the graph yet. The caller stores each
    /// as a waiting reference (`UnresolvedReference::waiting_operation`) that resolves to the
    /// edge when the record is ingested.
    pub unresolved: Vec<UnresolvedReference>,
}

/// The graph type of the bucket node that holds the references waiting on one record.
pub const UNRESOLVED_TYPE: &str = "UnresolvedReference";

/// The relationship of the edge from a changeset node to a bucket it waits on.
pub const AWAITS: &str = "awaits";

/// The bucket node id for the references that wait on the record `missing` (a graph node id):
/// one bucket per missing record, so ingesting the record reads exactly its waiters.
pub fn unresolved_node_id(missing: &str) -> String {
    format!("unresolved:{missing}")
}

/// A trailer-derived edge whose endpoint record is not ingested yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedReference {
    /// The edge as it will be written once the record exists, endpoints already graph ids.
    pub edge: DerivedEdge,
    /// The graph node id of the record the edge waits for.
    pub missing: String,
}

impl UnresolvedReference {
    /// The bucket node id this reference waits in.
    pub fn bucket_id(&self) -> String {
        unresolved_node_id(&self.missing)
    }

    /// The properties of the `awaits` edge from the changeset node to the bucket: the whole
    /// derived edge, so a later resolution reads nothing else.
    pub fn waiting_properties(&self) -> Vec<(&'static str, String)> {
        vec![
            ("relationship", AWAITS.to_owned()),
            ("edge_kind", self.edge.kind.name().to_owned()),
            ("edge_from", self.edge.from.clone()),
            ("edge_to", self.edge.to.clone()),
            ("rule", self.edge.rule.clone()),
            ("rule_version", self.edge.rule_version.to_string()),
            ("input_digests", self.edge.input_digests.join(",")),
        ]
    }

    /// Read a waiting edge back from the properties of its `awaits` edge. `None` when a
    /// property is missing or names no lifecycle edge kind.
    pub fn resolve(get: &dyn Fn(&str) -> Option<String>) -> Option<DerivedEdge> {
        let name = get("edge_kind")?;
        let kind = LifecycleEdgeKind::ALL.into_iter().find(|k| k.name() == name)?;
        Some(DerivedEdge {
            kind,
            from: get("edge_from")?,
            to: get("edge_to")?,
            rule: get("rule")?,
            rule_version: get("rule_version")?.parse().ok()?,
            input_digests: get("input_digests")?
                .split(',')
                .filter(|digest| !digest.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
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
/// record `requirement:<repo>:<row>`, the defect end of `fixedBy` and `introducedBy` is
/// `defect:<repo>/<id>`; every other endpoint already is a node id.
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
    // A defect is graph-unique by repository: `defect:<repo>/<id>`.
    let defect = from_side
        && matches!(kind, LifecycleEdgeKind::FixedBy | LifecycleEdgeKind::IntroducedBy);
    if requirement {
        format!("requirement:{}:{raw}", changeset.repo)
    } else if defect {
        format!("defect:{}/{raw}", changeset.repo)
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
/// (`citesRequirement`, `implements`, `fixedBy`) is held back and reported in `unresolved`, to
/// wait in a bucket until its record is ingested.
pub fn accept_changeset_edges(
    store: &mut LifecycleStore,
    changeset: &Changeset,
    view: &CodeGraphView,
    node: ChangesetNodeWrite,
    known: &dyn Fn(&str) -> Option<LifecycleKind>,
) -> Result<ChangesetWrites, LifecycleRefusal> {
    let mut kept = Vec::new();
    let mut waiting: Vec<(DerivedEdge, String)> = Vec::new();
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
                    waiting.push((edge.clone(), id));
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
    let unresolved = waiting
        .into_iter()
        .map(|(edge, missing)| UnresolvedReference {
            edge: DerivedEdge {
                from: graph_endpoint(changeset, edge.kind, true, &edge.from),
                to: graph_endpoint(changeset, edge.kind, false, &edge.to),
                ..edge
            },
            missing,
        })
        .collect();
    Ok(ChangesetWrites {
        node,
        edges,
        unresolved,
    })
}
