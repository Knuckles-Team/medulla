//! Declared results of the `transactions` contract domain.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::change_envelope::ChangeEnvelopeCommit;
use medulla_mutation_types::{
    mutation_outbox::MutationOutboxStatusView, mutation_outbox::OutboxAcked,
    mutation_outbox::OutboxDeadLetterPage, mutation_outbox::OutboxPulled,
    mutation_outbox::OutboxRewindReceipt,
};

/// What one SPARQL UPDATE changed, whichever path executed it.
///
/// `ApplyMutation` has two executors: the per-graph gateway (one graph, the update
/// runs against its staged image) and the coordinated SPARQL-HTTP saga (the
/// `sparql_http_update_v1` event, which may create and rewrite several graphs). Both
/// answer with this one body; a per-graph update reports `updated_graphs: 1` and
/// `created_graphs: 0`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SparqlUpdateReport {
    /// Update operations applied (one per `;`-separated clause).
    pub operations: u64,
    /// Triples inserted.
    pub inserted: u64,
    /// Triples deleted.
    pub deleted: u64,
    /// Graphs whose committed image this update replaced.
    pub updated_graphs: u64,
    /// Graphs this update created.
    pub created_graphs: u64,
    /// SHA-256 of the update request text, 64 lowercase hex characters
    /// (MEDULLA-FEDERATED-QUERY-R707.5). Empty only on a report sealed before the field existed.
    #[serde(default)]
    pub request_sha256: String,
    /// The id of the committed `MutationBatch` this update ran as, the receipt a reader looks
    /// up in the ledger (MEDULLA-FEDERATED-QUERY-R707.5). Empty for a per-graph update, which
    /// commits through its graph's own batch.
    #[serde(default)]
    pub batch_id: String,
}

/// The per-kind operation counts of one applied `BatchUpdate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BatchUpdateReport {
    pub added_nodes: u32,
    pub upserted_nodes: u32,
    pub removed_nodes: u32,
    pub added_edges: u32,
    pub upserted_edges: u32,
    pub removed_edges: u32,
    pub added_embeddings: u32,
    pub errors: Vec<String>,
}

impl BatchUpdateReport {
    /// Decode the MessagePack summary the batch executor produced.
    pub fn decode(summary: &[u8]) -> Result<Self, String> {
        medulla_types::msgpack::decode_bounded(
            summary,
            medulla_types::msgpack::MsgpackLimits::new(
                medulla_types::msgpack::MAX_PROPERTY_BYTES,
                medulla_types::msgpack::MAX_PROPERTY_ITEMS,
                medulla_types::msgpack::DEFAULT_MAX_DEPTH,
            ),
        )
        .map_err(|error| format!("invalid BatchUpdate summary: {error:?}"))
    }
}

/// A partial-success cross-graph batch: every requested graph appears exactly once,
/// in `results` (its `BatchUpdate` report) or in `errors`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MultiGraphBatchReport {
    pub results: BTreeMap<String, BatchUpdateReport>,
    pub errors: BTreeMap<String, String>,
    /// One entry per sub-batch in request order: the shard writer it queued
    /// on, its queue and commit times and its own outcome
    /// (MEDULLA-DURABLE-KERNEL-R099.5).
    #[serde(default)]
    pub fanout: Vec<crate::mutation_batch::ShardFanoutReport>,
}

/// The placement a replicated `ApplyChangeEnvelope` committed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeEnvelopeReplication {
    pub replicated: bool,
    pub group: u64,
    pub epoch: u64,
    pub fencing_token: u64,
}

/// A durably committed change envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeEnvelopeApplied {
    #[serde(flatten)]
    pub commit: ChangeEnvelopeCommit,
    /// The commit is durable but its in-memory projection has not been published.
    pub projection_pending: bool,
    /// Present only when the envelope was committed through a Raft placement group.
    #[serde(flatten)]
    pub replication: Option<ChangeEnvelopeReplication>,
    /// Present only when a stale version expectation was resolved by a merge
    /// strategy (MEDULLA-DURABLE-KERNEL-R084.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_conflict: Option<medulla_mutation_types::write_conflict::WriteConflictEvidence>,
}

/// An envelope of an `ApplyChangeEnvelopes` batch that did not commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeEnvelopeConflict {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
    pub error: String,
}

/// One envelope's outcome within an `ApplyChangeEnvelopes` batch, tagged by `result`:
/// what the envelope did in this call, final for the call, never a lifecycle
/// (MEDULLA-DURABLE-KERNEL-R875.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ChangeEnvelopeOutcome {
    Applied(ChangeEnvelopeApplied),
    IdempotentSkip(ChangeEnvelopeApplied),
    Conflict(ChangeEnvelopeConflict),
}

/// Per-envelope outcomes, in request order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeEnvelopeBatch {
    pub results: Vec<ChangeEnvelopeOutcome>,
}

/// The propagated belief a `TxnMaterializeBelief` stage froze into the transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BeliefMaterialization {
    pub node_id: String,
    pub confidence: f64,
}

/// A `Commit` outcome. Without a caller idempotency key the body is the bare
/// `committed` boolean; with one it also reports whether this answer replayed an
/// earlier commit under the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CommitOutcome {
    Unkeyed(bool),
    Keyed { committed: bool, replayed: bool },
}

method_results! {
    visit_transactions;
    BatchUpdate(BatchUpdate) => Json<BatchUpdateReport>;
    MultiGraphBatchUpdate(MultiGraphBatchUpdate) => Json<MultiGraphBatchReport>;
    ApplyChangeEnvelope(ApplyChangeEnvelope) => Json<ChangeEnvelopeApplied>;
    ApplyChangeEnvelopes(ApplyChangeEnvelopes) => Json<ChangeEnvelopeBatch>;
    ApplyChangeEnvelopeDraft(ApplyChangeEnvelopeDraft) => Json<ChangeEnvelopeApplied>;
    ApplyChangeEnvelopeDrafts(ApplyChangeEnvelopeDrafts) => Json<ChangeEnvelopeBatch>;
    ApplyMultisigMutation(ApplyMultisigMutation) => Json<SparqlUpdateReport>;
    BeginTxn(BeginTxn) => Text<String>;
    TxnAddNode(TxnAddNode) => Bool<bool>;
    TxnRemoveNode(TxnRemoveNode) => Bool<bool>;
    TxnAddEdge(TxnAddEdge) => Bool<bool>;
    TxnRemoveEdge(TxnRemoveEdge) => Bool<bool>;
    TxnCas(TxnCas) => Bool<bool>;
    TxnAddEmbedding(TxnAddEmbedding) => Bool<bool>;
    TxnBlobRef(TxnBlobRef) => Bool<bool>;
    TxnAddMeasurement(TxnAddMeasurement) => Bool<bool>;
    TxnAxiom(TxnAxiom) => Bool<bool>;
    TxnConstruct(TxnConstruct) => Bool<bool>;
    TxnPlanWriteback(TxnPlanWriteback) => Bool<bool>;
    TxnMaterializeBelief(TxnMaterializeBelief) => Json<BeliefMaterialization>;
    Commit(Commit) => Json<CommitOutcome>;
    Rollback(Rollback) => Bool<bool>;
    MutationOutboxStatus(MutationOutbox / "status") => Raw<MutationOutboxStatusView>;
    MutationOutboxDeadLetters(MutationOutbox / "dead_letters") => Raw<OutboxDeadLetterPage>;
    MutationOutboxRewind(MutationOutbox / "rewind") => Raw<OutboxRewindReceipt>;
    MutationOutboxPull(MutationOutbox / "pull") => Raw<OutboxPulled>;
    MutationOutboxAck(MutationOutbox / "ack") => Raw<OutboxAcked>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R875.4
    #[test]
    fn change_envelope_result_tag_is_not_a_lifecycle_field() {
        let conflict = ChangeEnvelopeOutcome::Conflict(ChangeEnvelopeConflict {
            envelope_id: Some("env-1".into()),
            error: "version expectation failed".into(),
        });
        let wire = serde_json::to_value(&conflict).unwrap();
        // The tag names what the envelope did in this call; the words are unchanged.
        assert_eq!(wire["result"], "conflict");
        assert!(wire.get("status").is_none());
        assert_eq!(
            serde_json::from_value::<ChangeEnvelopeOutcome>(wire).unwrap(),
            conflict
        );
        // Hard cutover: the old tag key does not decode.
        assert!(
            serde_json::from_value::<ChangeEnvelopeOutcome>(serde_json::json!({
                "status": "conflict",
                "envelope_id": "env-1",
                "error": "version expectation failed",
            }))
            .is_err()
        );
    }
}
