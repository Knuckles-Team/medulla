//! The query-side adapter on the served read paths (EH-396).
//!
//! One owner answers "which adapter applies to this query": the tenant's
//! adapter state for the graph's embedding space, read from the Agent Library
//! control owner on every vector-ranked read (so an activation or rollback is
//! seen by the next query, on every replica that shares the owner), with the
//! immutable, content-addressed body cached by digest.
//!
//! The adapter only ever changes the QUERY vector, and only where the probe is
//! already confined to what the caller may see: `SemanticSearch`/`Discover`
//! run over the caller's row-projected core, and a unified plan's `Rank` ranks
//! inside its RLS-filtered candidate allowlist. It therefore re-orders visible
//! rows and can never admit one. A space with no active adapter -- or an
//! adapter state that cannot be read -- serves the base query unchanged.

#[cfg(feature = "redb")]
use medulla_persistence::agent_library::AgentLibraryStateExt;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use medulla_core::graph::GraphCore;
use medulla_types::decision::statistical::retrieval_adapter::AdapterFitted;

use medulla_server_api::api::admin::served_adapter::ServedAdapter;
use medulla_persistence::agent_library::AgentLibraryStore;
use medulla_persistence::decision_jobs::decode_artifact;
use medulla_server_api::state::ServerState;

/// Bodies kept decoded; the cache is cleared when it would grow past this.
const MAX_CACHED_BODIES: usize = 32;

/// Key of a fitted adapter (body + receipt).
pub(crate) fn adapter_key(adapter_digest: &str) -> String {
    format!("adapter:{adapter_digest}")
}

/// A fitted adapter stored under `tenant`.
pub(crate) fn read_fitted(
    store: &AgentLibraryStore,
    tenant: &str,
    adapter_digest: &str,
) -> Result<Option<AdapterFitted>, String> {
    store
        .decision_artifact(tenant, &adapter_key(adapter_digest))?
        .map(|bytes| decode_artifact(&bytes, "query adapter"))
        .transpose()
}

fn body_cache() -> &'static parking_lot::Mutex<BTreeMap<String, Arc<ServedAdapter>>> {
    static CACHE: OnceLock<parking_lot::Mutex<BTreeMap<String, Arc<ServedAdapter>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| parking_lot::Mutex::new(BTreeMap::new()))
}

fn cached_or_decode(
    store: &AgentLibraryStore,
    tenant: &str,
    digest: &str,
) -> Result<Option<Arc<ServedAdapter>>, String> {
    if let Some(hit) = body_cache().lock().get(digest) {
        return Ok(Some(Arc::clone(hit)));
    }
    let Some(fitted) = read_fitted(store, tenant, digest)? else {
        return Ok(None);
    };
    let served = Arc::new(ServedAdapter::for_body(digest, &fitted.body)?);
    let mut cache = body_cache().lock();
    if cache.len() >= MAX_CACHED_BODIES {
        cache.clear();
    }
    cache.insert(digest.to_string(), Arc::clone(&served));
    Ok(Some(served))
}

fn load(
    store: &AgentLibraryStore,
    tenant: &str,
    graph: &str,
) -> Result<Option<Arc<ServedAdapter>>, String> {
    let key = super::stat_adapter::adapter_pointer(graph);
    let (_, pointer) = super::stat_pointer::read_pointer(store, tenant, &key)?;
    match pointer.target() {
        Some(digest) => cached_or_decode(store, tenant, digest),
        None => Ok(None),
    }
}

/// The adapter active for `graph` under the verified carrier tenant
/// `tenant`. `None` when there is no carrier, the Agent Library was never
/// opened, the store declares a space other than the adapter's, or the
/// state cannot be read (logged; the base query is served).
pub(crate) async fn served_for(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    tenant: Option<&str>,
    graph: &str,
    core: &GraphCore,
) -> Option<Arc<ServedAdapter>> {
    let (tenant, graph) = (tenant?.to_string(), graph.to_string());
    let store = state.read().await.agent_library()?;
    let loaded = tokio::task::spawn_blocking(move || load(&store, &tenant, &graph)).await;
    let declared = core.semantic_store.read().space().map(|s| s.digest.clone());
    let in_space = |a: &Arc<ServedAdapter>| declared.as_ref().is_none_or(|d| d == a.space_digest());
    match loaded {
        Ok(Ok(adapter)) => adapter.filter(in_space),
        Ok(Err(error)) => {
            tracing::warn!(%error, "query adapter state unreadable; serving the base query");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "query adapter load task failed; serving the base query");
            None
        }
    }
}
