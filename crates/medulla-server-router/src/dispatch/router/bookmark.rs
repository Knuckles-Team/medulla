//! `Method::BookmarkSemanticSearch` (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! `medulla-h-retrieve` has no access to the caller's row-projected core, so this handler reads
//! what the search needs from it once, at the start of the call: the core's version, the policy
//! revision and the visibility and property reads. The search itself lives in
//! `medulla_h_retrieve::bookmark::search`.

use super::*;

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_protocol_types::result_contract::query::BookmarkSemanticSearch;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::shared::{
    authorized_graph_read, ensure_graph_resident, timed_read, typed_response,
};
use medulla_types::bookmark_query::BookmarkSemanticSearchRequest;
use tokio::sync::RwLock;

/// Serve one bookmark search over the caller's projected core and the `bookmark` semantic index.
pub(super) async fn handle_bookmark_search(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: BookmarkSemanticSearchRequest,
) -> Response {
    let result = async {
        let authority = CarrierAuthority::from_verified(verified)?;
        if !authority.can_read() {
            return Err("ACCESS_DENIED: bookmark search requires kg:read".to_string());
        }
        ensure_graph_resident(state, graph).await;
        let (core, read, authorization_revision, persist_dir) = {
            let current = timed_read(state).await;
            let (core, read) = authorized_graph_read(
                &current,
                graph,
                verified,
                &format!("GRAPH_NOT_FOUND: {graph}"),
            )?;
            #[cfg(feature = "security")]
            let authorization_revision = current.isolation.policy_revision();
            #[cfg(not(feature = "security"))]
            let authorization_revision = 0;
            let persist_dir = current.persist_dir.as_deref().map(std::path::PathBuf::from).ok_or_else(
                || "ENGINE_UNAVAILABLE: semantic index storage is unavailable".to_string(),
            )?;
            (core, read, authorization_revision, persist_dir)
        };
        let service = medulla_h_retrieve::semantic_index_runtime::open_semantic_service(
            &persist_dir,
            authority.tenant_scope(),
            medulla_h_retrieve::bookmark::BOOKMARK_INDEX_BINDING_ID,
        )
        .map_err(|_| "ENGINE_UNAVAILABLE: semantic index storage is unavailable".to_string())?;
        // The index is built from one graph text property; its hits name source entities, which
        // map back to the nodes of that type deterministically.
        let selector = medulla_h_retrieve::bookmark::bookmark_graph_selector(&service)?;
        if selector.graph_id != graph {
            return Err(medulla_h_retrieve::bookmark::refusal(
                medulla_types::bookmark_query::BOOKMARK_INDEX_STALE,
                "the bookmark index is built from another graph",
            ));
        }
        // One version for every read of the call.
        let graph_snapshot_revision = core.version();
        let mut node_ids = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = core.get_nodes_by_label_page(&selector.type_id, after.as_deref(), 1024);
            let full = page.len() == 1024;
            after = page.last().map(|(id, _)| id.clone());
            node_ids.extend(page.into_iter().map(|(id, _)| id));
            if !full {
                break;
            }
        }
        let entities = medulla_h_retrieve::bookmark::entity_node_map(
            &selector,
            authority.tenant_scope(),
            node_ids,
        );
        let node_of_entity = |entity: &str| entities.get(entity).cloned();
        let visible = |id: &str| {
            core.get_node_properties(id)
                .is_some_and(|blob| read.can_see_node(&blob, core.is_schema_node(id)))
        };
        let properties = |id: &str| -> Option<BTreeMap<String, String>> {
            let blob = core.get_node_properties(id)?;
            let decoded = medulla_types::msgpack::decode_property_object(&blob).ok()?;
            Some(
                decoded
                    .into_iter()
                    .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
                    .collect(),
            )
        };
        let view = medulla_h_retrieve::bookmark::BookmarkReadView {
            graph_snapshot_revision,
            authorization_revision,
            tenant: authority.tenant_scope(),
            node_of_entity: &node_of_entity,
            visible: &visible,
            properties: &properties,
        };
        medulla_h_retrieve::bookmark::search(&service, &request, &view)
    }
    .await;
    typed_response::<BookmarkSemanticSearch>(req_id, result)
}
