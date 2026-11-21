//! A write a handler has decided on but does not commit itself
//! (MEDULLA-DECISION-ENGINE-R701.3, reused by R702.3).
//!
//! The h-decide crate cannot reach the router's graph commit, so a handler that
//! owns a graph write answers the batch it wants committed and the reply the
//! caller gets once the commit succeeds. The router encodes `operations` as one
//! `BatchUpdate` against `graph` under the caller's verified context; a refused
//! commit answers the refusal and never the reply, and an empty list commits
//! nothing and answers the reply.

use medulla_protocol_types::protocol::ResultPayload;

/// The batch a handler plans and the reply that follows its commit.
pub struct PlannedWrite {
    /// The graph the operations commit to.
    pub graph: String,
    /// `BatchUpdate` operations (`upsert_node`, `upsert_edge`, ...), in order.
    pub operations: Vec<serde_json::Value>,
    /// What the caller is answered once the commit succeeds.
    pub reply: ResultPayload,
}
