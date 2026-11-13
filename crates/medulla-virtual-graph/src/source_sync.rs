//! Source sync: the pure three-way reconcile rule and the adapter trait
//! (MEDULLA-SRC-AUTH-R012.3), and the sync state written as graph facts
//! (MEDULLA-SRC-AUTH-R012.4).
//!
//! A federated secondary is kept in step with the native graph, the system of
//! record. For each record the reconcile compares three digests: the base (the
//! record as of the last sync), the graph value and the source value. The
//! function is pure; the served handler reads the source, calls [`reconcile`]
//! per record and writes one batch holding the inbound changes and the sync
//! state together.

mod plan;
mod state;

use serde_json::{Map, Value};

pub use plan::{base_digests, plan_sync, GraphNodes, SyncPlan};
pub use state::{sync_state_mutations, SyncOutcome, SYNC_STATE_CLASS};

/// What happens when both sides changed a record (`vg:conflictRule`). The set
/// is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictRule {
    /// The graph value wins and is proposed outbound.
    SystemOfRecordWins,
    /// Nothing is proposed; the conflict waits for a person.
    HoldForReview,
}

impl ConflictRule {
    /// The `vg:conflictRule` literal.
    pub fn parse(literal: &str) -> Option<Self> {
        match literal {
            "system_of_record_wins" => Some(Self::SystemOfRecordWins),
            "hold_for_review" => Some(Self::HoldForReview),
            _ => None,
        }
    }
}

/// The role a source plays for a class (`vg:recordRole`). The set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordRole {
    /// The native graph: the one source of truth for the class.
    SystemOfRecord,
    /// A secondary kept in step with the system of record.
    FederatedSecondary,
}

impl RecordRole {
    /// The `vg:recordRole` literal.
    pub fn parse(literal: &str) -> Option<Self> {
        match literal {
            "system_of_record" => Some(Self::SystemOfRecord),
            "federated_secondary" => Some(Self::FederatedSecondary),
            _ => None,
        }
    }
}

/// Which way changes may flow between a secondary and the graph
/// (`vg:syncDirection`). The set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    /// Source changes may be applied to the graph; nothing is proposed back.
    Inbound,
    /// Graph changes may be proposed to the source; nothing is applied.
    Outbound,
    /// Both ways.
    Both,
}

impl SyncDirection {
    /// The `vg:syncDirection` literal.
    pub fn parse(literal: &str) -> Option<Self> {
        match literal {
            "inbound" => Some(Self::Inbound),
            "outbound" => Some(Self::Outbound),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Whether a source change may be applied to the graph.
    pub fn allows_inbound(self) -> bool {
        matches!(self, Self::Inbound | Self::Both)
    }

    /// Whether a graph change may be proposed to the source.
    pub fn allows_outbound(self) -> bool {
        matches!(self, Self::Outbound | Self::Both)
    }
}

/// One `vg:SourceAuthority` row read for a sync: which connection plays which
/// role for which class, and how a two-sided change is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAuthority {
    /// The authority row's IRI.
    pub row: String,
    /// The IRI of `vg:authoritativeSource`.
    pub connection: String,
    /// The IRI of `vg:authorityFor`.
    pub class: String,
    /// `vg:recordRole`.
    pub role: RecordRole,
    /// `vg:syncDirection`; present on a federated secondary.
    pub direction: Option<SyncDirection>,
    /// The `vg:conflictRule` literal; present on a federated secondary.
    pub conflict_rule: Option<String>,
}

/// One side of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The graph side.
    Graph,
    /// The external source side.
    Source,
}

/// The decision for one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Both sides agree with the base (or with each other).
    Unchanged,
    /// Only the source changed: write the source value into the graph.
    ApplyInbound,
    /// Only the graph changed: propose the graph value to the source.
    ProposeOutbound,
    /// Both changed. `kept` is the side whose value stands (and is proposed
    /// outbound when it is the graph); `held` are the sides held back.
    Conflict {
        /// The side whose value stands, if one was chosen.
        kept: Option<Side>,
        /// The sides held back.
        held: Vec<Side>,
    },
}

impl SyncAction {
    /// Whether the graph value is proposed to the source.
    pub fn proposes_outbound(&self) -> bool {
        match self {
            Self::ProposeOutbound => true,
            Self::Conflict { kept, .. } => *kept == Some(Side::Graph),
            Self::Unchanged | Self::ApplyInbound => false,
        }
    }
}

/// A node upsert: the node id and its property map. Inbound changes and sync
/// state are both expressed this way so they ride one batch.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncMutation {
    /// The id of the node to upsert.
    pub node_id: String,
    /// The property map to write on that node.
    pub properties: Map<String, Value>,
}

/// What one class of records supplies to a sync.
pub trait SyncAdapter {
    /// The stable key of a record, or `None` when it has none.
    fn record_key(&self, record: &Value) -> Option<String>;
    /// The digest of the fields the sync compares.
    fn record_digest(&self, record: &Value) -> String;
    /// The graph writes that apply a source record inbound.
    fn inbound_mutations(&self, key: &str, record: &Value) -> Vec<SyncMutation>;
    /// The payload proposed to the source for a graph record.
    fn outbound_payload(&self, key: &str, record: &Value) -> Value;
    /// The graph side of the sync: every visible record of the class, keyed
    /// like `record_key` and shaped like a source record, rebuilt from the
    /// visible nodes.
    fn graph_records(&self, nodes: &plan::GraphNodes) -> std::collections::BTreeMap<String, Value>;
}

/// The three-way rule over `base`, `graph` and `source` digests (`None` is
/// absent). Equal graph and source give `Unchanged`; only the source changed
/// gives `ApplyInbound`; only the graph changed gives `ProposeOutbound`; both
/// changed gives `Conflict`. A record absent from the source and unchanged in
/// the graph proposes a re-create outbound.
pub fn reconcile(
    base: Option<&str>,
    graph: Option<&str>,
    source: Option<&str>,
    rule: ConflictRule,
) -> SyncAction {
    if graph == source {
        return SyncAction::Unchanged;
    }
    if source.is_none() && graph == base {
        return SyncAction::ProposeOutbound;
    }
    match (graph != base, source != base) {
        (false, _) => SyncAction::ApplyInbound,
        (true, false) => SyncAction::ProposeOutbound,
        (true, true) => match rule {
            ConflictRule::SystemOfRecordWins => SyncAction::Conflict {
                kept: Some(Side::Graph),
                held: vec![Side::Source],
            },
            ConflictRule::HoldForReview => SyncAction::Conflict {
                kept: None,
                held: vec![Side::Graph, Side::Source],
            },
        },
    }
}

/// The digest the host opens the foreign read with for one connection and
/// class, and the served handler recomputes to take the read's rows exactly
/// once: SHA-256 over `connection_iri`, a zero byte, then `class_iri`.
pub fn sync_plan_digest(
    connection_iri: &str,
    class_iri: &str,
) -> medulla_types::contract::Digest256 {
    let mut framed = Vec::new();
    framed.extend_from_slice(connection_iri.as_bytes());
    framed.push(0);
    framed.extend_from_slice(class_iri.as_bytes());
    medulla_types::contract::Digest256::sha256(&framed)
}

/// The inbound writes and the sync state of one sync, as the one list a single
/// mutation batch commits. A failed batch leaves both unchanged.
pub fn sync_batch(inbound: Vec<SyncMutation>, state: Vec<SyncMutation>) -> Vec<SyncMutation> {
    inbound.into_iter().chain(state).collect()
}

#[cfg(test)]
mod tests;
