//! The graph text-property source of a semantic index (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! The semantic-index handler lives in `medulla-h-retrieve`, which has no access to the caller's
//! row-projected core. This capability reads it here, once per source operation: the core's
//! version first, then every node of the selector's type the caller may see, keyed by node id.

use std::sync::Arc;

use medulla_h_retrieve::semantic_index_runtime::graph_source::{
    GraphSourceFuture, GraphSourceProvider, GraphSourceSnapshot, MAX_GRAPH_SOURCE_NODES,
};
use medulla_index_types::semantic_index::GraphTextPropertyRef;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::shared::{
    authorized_graph_read, ensure_graph_resident, timed_read,
};
use tokio::sync::RwLock;

/// Nodes read from the core per keyset page.
const SCAN_PAGE: usize = 1024;

/// Reads graph sources over the verified caller's projected core.
pub struct RouterGraphSource<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub verified: &'a VerifiedRequestContext,
}

impl GraphSourceProvider for RouterGraphSource<'_> {
    fn snapshot<'a>(&'a self, selector: &'a GraphTextPropertyRef) -> GraphSourceFuture<'a> {
        Box::pin(async move {
            let graph = selector.graph_id.as_str();
            ensure_graph_resident(self.state, graph).await;
            let (core, read) = {
                let current = timed_read(self.state).await;
                authorized_graph_read(
                    &current,
                    graph,
                    self.verified,
                    &format!("GRAPH_NOT_FOUND: {graph}"),
                )?
            };
            // The version is read before the scan, so the epoch is a lower bound of what the
            // nodes reflect; a later commit only advances it.
            let graph_version = core.version();
            let mut nodes: Vec<(String, String)> = Vec::new();
            let mut after: Option<String> = None;
            loop {
                let page =
                    core.get_nodes_by_label_page(&selector.type_id, after.as_deref(), SCAN_PAGE);
                let Some((last, _)) = page.last() else { break };
                after = Some(last.clone());
                let full = page.len() == SCAN_PAGE;
                for (id, blob) in page {
                    if !read.can_see_node(&blob, core.is_schema_node(&id)) {
                        continue;
                    }
                    let Ok(properties) = medulla_types::msgpack::decode_property_object(&blob)
                    else {
                        continue;
                    };
                    let Some(text) = properties
                        .get(&selector.property_id)
                        .and_then(|value| value.as_str())
                        .filter(|text| !text.is_empty())
                    else {
                        continue;
                    };
                    nodes.push((id, text.to_string()));
                }
                if nodes.len() > MAX_GRAPH_SOURCE_NODES {
                    return Err(format!(
                        "RESOURCE_EXHAUSTED: graph source exceeds {MAX_GRAPH_SOURCE_NODES} nodes"
                    ));
                }
                if !full {
                    break;
                }
            }
            Ok(GraphSourceSnapshot {
                selector: selector.clone(),
                graph_version,
                nodes,
            })
        })
    }
}
