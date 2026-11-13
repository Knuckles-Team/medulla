//! `TraceCritique`: the authorized read of a stored recursive reasoning trace
//! (MEDULLA-DECISION-ENGINE-R133.7.3).
//!
//! One graph snapshot supplies every node of the page, so the selected nodes,
//! their ancestry and their source revisions are read at the same version. The
//! walk, the paging and the checks are the pure
//! [`medulla_decision_types::decision::recursive_trace::read_critique`]; this
//! module only supplies the tenant-bound node lookup.
use super::*;
use medulla_decision_types::decision::recursive_trace::{
    read_critique, RecursiveTraceNode, TraceCritiquePage, TraceCritiqueQuery,
};

/// The class label a stored recursive trace node carries.
const RECURSIVE_TRACE_CLASS: &str = "RecursiveTraceNode";
/// The property of a stored node that carries its `RecursiveTraceNode` record.
const RECURSIVE_TRACE_RECORD: &str = "recursive_trace";

/// Read one critique page for `query.tenant`. A node of another tenant, or a
/// row that is not a recursive trace node, reads as absent, so a foreign root
/// is refused exactly like a missing one.
pub fn read_trace_critique(
    shard: &Shard,
    graph: &str,
    query: &TraceCritiqueQuery,
    content_authorized: bool,
    page_limit: u32,
    crypto: DurableCrypto<'_>,
) -> Result<TraceCritiquePage, String> {
    let handle = shard.graph(graph)?;
    let snapshot = shard.read(&handle)?;
    let nodes = snapshot.scoped_owner_table(NODES)?;
    read_critique(query, content_authorized, page_limit, |node_id| {
        let Some(bytes) = nodes.get((graph, node_id))? else {
            return Ok(None);
        };
        let row: serde_json::Map<String, serde_json::Value> =
            decode_durable(&crypto.unseal(bytes.value())?)?;
        let ours = row.get("tenant").and_then(serde_json::Value::as_str)
            == Some(query.tenant.as_str())
            && row.get("node_type").and_then(serde_json::Value::as_str)
                == Some(RECURSIVE_TRACE_CLASS);
        if !ours {
            return Ok(None);
        }
        let record = row
            .get(RECURSIVE_TRACE_RECORD)
            .ok_or_else(|| format!("stored trace node '{node_id}' carries no record"))?;
        let node: RecursiveTraceNode = serde_json::from_value(record.clone())
            .map_err(|error| format!("stored trace node '{node_id}' does not decode: {error}"))?;
        Ok(Some(node))
    })
    .map_err(|error| error.into_wire())
}
