//! The `bookmark` semantic binding at connector pack activation (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! `medulla-h-admin` does not depend on `medulla-h-retrieve`, so the router, which depends on
//! both, is where a pack activation reaches the semantic index. After an import or a held
//! release activates a pack, this reads whether the connector's current pack declares the
//! `Bookmark` class; when it does, it reads the bound text embedder's registered space, the
//! isolation layer's policy revision and the activating request's graph, and has the retrieval
//! crate author the tenant's `bookmark` binding. A pack that does not declare `Bookmark` changes
//! nothing, and a tenant that already has the binding keeps it.

use super::*;

use std::sync::Arc;

use medulla_h_retrieve::bookmark_binding::{
    author_bookmark_binding, BookmarkBindingInputs, BOOKMARK_NODE_TYPE,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// Author the `bookmark` binding of the verified tenant when `connector`'s current pack declares
/// the `Bookmark` class. `graph` is the activating request's graph.
pub(super) async fn author_on_pack_activation(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    verified: &VerifiedRequestContext,
    connector: &str,
) -> Result<(), String> {
    let connector =
        medulla_types::contract::ResourceId::new(connector).map_err(|error| error.to_string())?;
    let store = medulla_persistence_records::agent_library::shared_agent_library(state).await?;
    let declares = store.connector_declares_class(
        medulla_persistence_records::connector_pack::pack_tenant_key(verified),
        &connector,
        BOOKMARK_NODE_TYPE,
    )?;
    if !declares {
        return Ok(());
    }
    // With no text embedder bound there is no registered space to embed into, so no binding can
    // be authored. The import itself stays valid; the next activation of the pack authors it,
    // and a search before then refuses `BOOKMARK_INDEX_STALE` (no binding).
    let state_embedder = state.read().await.text_embedder_override.clone();
    let Some(identity) =
        medulla_server_ops::api::query_txn::query_support::state_text_encoder_identity(
            state_embedder.as_ref(),
        )
    else {
        tracing::warn!(
            "the pack declares Bookmark but no text embedder is bound; the bookmark index binding was not authored"
        );
        return Ok(());
    };
    let policy_revision = {
        let current = state.read().await;
        #[cfg(feature = "security")]
        let revision = current.isolation.policy_revision();
        #[cfg(not(feature = "security"))]
        let revision = 0;
        revision
    };
    let authority = CarrierAuthority::from_verified(verified)?;
    let inputs = BookmarkBindingInputs {
        graph_id: graph.to_string(),
        space: medulla_server_ops::api::query_txn::query_support::pin_embedding_space(&identity)?,
        policy_revision,
        now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    };
    author_bookmark_binding(state, &authority, &inputs)
        .await
        .map(|_| ())
}
