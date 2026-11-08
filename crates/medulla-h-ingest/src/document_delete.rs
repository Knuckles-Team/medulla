//! Lowering of a planned Document delete cascade (MEDULLA-UNIFIED-DATA-PLANE-R651.3/.5).
//!
//! `apply_cascade` in the server's graph pipeline hands a committed-to plan
//! here. The plan's edges and nodes become `RemoveEdge` / `RemoveNode`
//! methods, and one content-free `DocumentDeletion` node is added in the same
//! write-set, so the delete and its record commit as one batch or not at all.

use medulla_document::cascade::CascadePlan;
use medulla_protocol_types::protocol::Method;
use medulla_query_types::document_ops::{
    DocumentDeleteReceipt, DocumentDeletion, DOCUMENT_DELETION_TYPE,
};
use serde_json::json;

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

const ENVELOPE_PREFIX: &str = "document-delete:";
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "document-delete-sha256",
    policy_version: "document-delete",
};

/// Id of the deletion record of one plan.
pub fn deletion_id(plan: &CascadePlan) -> String {
    format!("document-deletion:{}", plan.plan_digest)
}

/// The record and receipt a plan yields once committed at `deleted_at` by `actor`.
pub fn deletion_record(
    plan: &CascadePlan,
    deleted_at: u64,
    actor: &str,
) -> (DocumentDeletion, DocumentDeleteReceipt) {
    let record = DocumentDeletion {
        document_id: plan.document_id.clone(),
        plan_digest: plan.plan_digest.clone(),
        nodes_deleted: plan.nodes.len() as u64,
        edges_deleted: plan.edges.len() as u64,
        deleted_at,
        actor: actor.to_string(),
    };
    let receipt = DocumentDeleteReceipt {
        tenant: plan.tenant.clone(),
        plan_digest: plan.plan_digest.clone(),
        nodes_deleted: record.nodes_deleted,
        edges_deleted: record.edges_deleted,
        index_pending: 0,
    };
    (record, receipt)
}

/// Lower the plan and its record to one digest-keyed write-set: edges first,
/// then nodes, then the deletion record node.
pub fn lower(plan: &CascadePlan, record: &DocumentDeletion) -> Result<IndexWriteSet, String> {
    let mut methods = Vec::with_capacity(plan.edges.len() + plan.nodes.len() + 1);
    for edge in &plan.edges {
        methods.push(Method::RemoveEdge {
            source_id: edge.from.id.clone(),
            target_id: edge.to.id.clone(),
        });
    }
    for node in &plan.nodes {
        methods.push(Method::RemoveNode {
            node_id: node.id.clone(),
        });
    }
    let properties = json!({
        "type": DOCUMENT_DELETION_TYPE,
        "document_id": record.document_id,
        "plan_digest": record.plan_digest,
        "nodes_deleted": record.nodes_deleted,
        "edges_deleted": record.edges_deleted,
        "deleted_at": record.deleted_at,
        "actor": record.actor,
    });
    methods.push(Method::AddNode {
        node_id: deletion_id(plan),
        properties_msgpack: rmp_serde::to_vec_named(&properties)
            .map_err(|error| format!("deletion record encoding failed: {error}"))?,
    });
    IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/document-delete", methods)
}
