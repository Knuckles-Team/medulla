//! The commit publication seam (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
//!
//! Every durable batch commit (compact, staged, cross-modal, top-up, change
//! envelope) ends in [`super::shard::Shard::commit_batch`], and every other write
//! that bumps a graph version (a coalesced `commit_ops` drain, a maintenance
//! write) ends in [`super::shard::Shard::commit_drain_with`]. Both share one
//! publication seam: the receipt is final in the open transaction, the
//! transaction is not yet committed, and a publisher refusal aborts the
//! transaction. A commit is therefore either published and durable locally, or
//! neither: a lost publish fence is never acked, and no version bump goes
//! unpublished.

use medulla_protocol_types::change_envelope::ChangeEnvelope;
use medulla_protocol_types::mutation_batch::{MutationBatch, MutationBatchRecord};
use medulla_protocol_types::protocol::{GraphType, Method};
use serde::{Deserialize, Serialize};

use crate::MeasurementBatch;

/// The out-of-line material a commit carries beside its batch, so a replay
/// re-commits through the same path the original took.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitMaterial {
    /// A compact batch: the batch is the whole effect.
    Compact,
    /// A staged batch: the authenticated graph state (snapshot or row delta)
    /// the batch's opaque operations stand for.
    Staged {
        authoritative_state_msgpack: Vec<u8>,
        audited: bool,
    },
    /// A cross-modal batch: the non-graph rows committed with it.
    CrossModal {
        methods: Vec<Method>,
        vectors: Vec<(String, Vec<f32>)>,
        blob_refs: Vec<(String, String)>,
        measurements: Vec<MeasurementBatch>,
    },
    /// A governed change-envelope commit: the envelope and the repository
    /// budget authority committed with it, so the replay re-commits the
    /// envelope, budget and governance rows the batch alone does not carry.
    ChangeEnvelope {
        envelope: ChangeEnvelope,
        source_budget: Option<crate::redb_store::enrichment_budget::SourceBudgetAuthority>,
    },
    /// A repository enrichment top-up: the MessagePack of the sealed
    /// `EnrichmentTopUpTransition`, whose source supersession and budget rows
    /// the replacement batch alone does not carry.
    TopUp { transition_msgpack: Vec<u8> },
    /// A coalesced drain of graph methods (`commit_ops`): the methods this
    /// graph's member applied in the one group, in order.
    Drain { methods: Vec<Method> },
    /// A maintenance write that bumped the graph version: the version bump is
    /// the replayed effect. The rows such a write leaves are node-local
    /// bookkeeping (audit chain, capability and budget rows) and are not part
    /// of the published graph.
    VersionBump,
}

/// One record of a published `BatchSegment`: the caller's batch as committed,
/// and the material it was committed with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSegmentEntry {
    pub record: MutationBatchRecord,
    pub material: CommitMaterial,
}

/// What a commit hands the chokepoint to publish.
pub struct CommitPublication<'a> {
    pub graph_fname: &'a str,
    /// The caller's batch before it is bound to the shard identity.
    pub caller_batch: &'a MutationBatch,
    pub material: CommitMaterial,
}

/// One commit as the publisher sees it, in the open transaction.
pub struct PublishedCommit<'a> {
    pub graph_fname: &'a str,
    /// The graph's durable name and type, read from its graph catalog row.
    pub graph_name: &'a str,
    pub graph_type: GraphType,
    /// The graph version this commit leaves (`None` when the commit moves no
    /// graph version).
    pub graph_version: Option<u64>,
    /// The graph version before this commit.
    pub prior_graph_version: u64,
    pub entry: &'a BatchSegmentEntry,
}

/// Publishes a commit before it is final. An `Err` aborts the commit.
pub trait CommitPublisher: Send + Sync {
    fn publish(&self, commit: &PublishedCommit<'_>) -> Result<(), String>;
}
