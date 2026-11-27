//! The graph reads and writes a catalog handler makes through the ordinary graph
//! gateway (`dispatch_graph_op`), so every write is ACL-checked, durable, audited
//! and CDC-emitted (MEDULLA-TYPED-PACKS-R801.3, R802.3).

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

use super::graph_pipeline::dispatch_graph_op;

/// A node's properties as stored.
pub type Properties = serde_json::Map<String, serde_json::Value>;

/// Who asked and which graph holds the records.
pub struct GraphTarget<'a> {
    /// The request graph.
    pub graph: &'a str,
    /// The request id.
    pub req_id: u64,
    /// The calling agent, when named.
    pub caller: Option<&'a str>,
    /// The verified request carrier.
    pub verified: &'a VerifiedRequestContext,
}

/// The property that holds the owning tenant on a catalog node.
pub const TENANT_PROPERTY: &str = "tenant_id";

/// Read one node's properties; `None` when the node does not exist.
pub async fn read_node(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    node_id: &str,
) -> Result<Option<Properties>, String> {
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_property_object(&bytes)
            .map(Some)
            .map_err(|_| "INVALID_ARGUMENT: the node properties are malformed".to_string()),
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => Ok(Some(object)),
        _ => Ok(None),
    }
}

/// Commit one write method; a refusal is the error.
pub async fn write(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    method: Method,
) -> Result<Response, String> {
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        method,
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(response),
    }
}

/// Encode a property map or condition map as msgpack.
pub fn encode(value: &Properties) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("INVALID_ARGUMENT: record encoding failed: {error}"))
}

/// Whether a write answer means the write took effect: a failed
/// compare-and-set answers `false`.
pub fn took_effect(response: &Response) -> bool {
    !matches!(
        &response.result,
        Some(ResultPayload::Bool(false))
            | Some(ResultPayload::Json(serde_json::Value::Bool(false)))
    )
}

/// Commit `operations` as one `BatchUpdate`.
pub async fn write_batch(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    operations: &[serde_json::Value],
) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(operations)
        .map_err(|error| format!("INVALID_ARGUMENT: batch encoding failed: {error}"))?;
    write(state, target, Method::BatchUpdate { operations_msgpack })
        .await
        .map(drop)
}
