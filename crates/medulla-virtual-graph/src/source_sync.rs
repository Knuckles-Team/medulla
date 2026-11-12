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

mod state;

use serde_json::{Map, Value};

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

/// One side of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Graph,
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
    Conflict { kept: Option<Side>, held: Vec<Side> },
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
    pub node_id: String,
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

/// The inbound writes and the sync state of one sync, as the one list a single
/// mutation batch commits. A failed batch leaves both unchanged.
pub fn sync_batch(inbound: Vec<SyncMutation>, state: Vec<SyncMutation>) -> Vec<SyncMutation> {
    inbound.into_iter().chain(state).collect()
}

#[cfg(test)]
mod tests;
