//! The usage prior of the ranked catalog function (MEDULLA-TYPED-PACKS-R108.16).
//!
//! There is one usage store: the `asset_usage.reads` series that the server's own query
//! dispatch writes (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1). A served SPARQL read loads the
//! caller's tenant and graph counts into the projection, and `cat:relevance` fuses them as
//! its capped prior. Nothing is copied onto the asset node.

use std::sync::Arc;

use medulla_catalog_types::catalog_relevance::UsageIndex;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// Whether `query` calls one of the catalog ranking functions.
pub(super) fn uses_catalog_ranking(query: &str) -> bool {
    use medulla_catalog_types::catalog_relevance::{CAT_RANK_COMPONENTS_IRI, CAT_RELEVANCE_IRI};
    [CAT_RELEVANCE_IRI, CAT_RANK_COMPONENTS_IRI]
        .iter()
        .any(|iri| query.contains(iri))
        || query.contains("relevance(")
        || query.contains("rankComponents(")
}

/// The reads per asset in the caller's tenant and graph. A deployment without a series store,
/// or a caller without verified tenant authority, has none.
#[cfg(feature = "tsdb")]
pub(super) async fn load_usage(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
) -> Option<Arc<UsageIndex>> {
    use medulla_catalog_types::asset_usage::ASSET_USAGE_READS_SERIES;
    use medulla_tsdb::store::SeriesKey;

    let carrier = read_authority.and_then(|authority| authority.carrier())?;
    let tenant = carrier.tenant_scope();
    let store = state.read().await.tsdb_store.clone()?;
    let catalog = store.scoped_catalog(tenant, graph_name).ok()?;
    let mut series = Vec::new();
    for name in catalog.into_keys() {
        if !name.starts_with(ASSET_USAGE_READS_SERIES) {
            continue;
        }
        let key = SeriesKey::new(tenant, graph_name, name.clone());
        let Ok(points) = store.scan_all_scoped(&key) else {
            continue;
        };
        let total: f64 = points
            .iter()
            .map(|p| p.values.first().copied().unwrap_or(0.0))
            .sum();
        series.push((name, total.max(0.0) as u64));
    }
    Some(Arc::new(medulla_catalog_types::catalog_relevance::usage_index(
        &series,
    )))
}

/// Without the series build no usage exists.
#[cfg(not(feature = "tsdb"))]
pub(super) async fn load_usage(
    _state: &Arc<RwLock<ServerState>>,
    _graph_name: &str,
    _read_authority: Option<&GraphReadAuthority>,
) -> Option<Arc<UsageIndex>> {
    None
}
