//! The usage prior of the ranked catalog function (MEDULLA-TYPED-PACKS-R108.16).
//!
//! There is one usage store: the `asset_usage.reads` series that the server's own query
//! dispatch writes (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1). A served SPARQL or SQL read that
//! ranks assets loads the caller's tenant and graph counts here, at query time, and hands them
//! to the one scorer in `medulla_catalog_types::catalog_relevance`. Nothing is copied onto the
//! asset node.

use std::sync::Arc;

use medulla_catalog_types::catalog_relevance::UsageIndex;
use tokio::sync::RwLock;

use crate::carrier::CarrierAuthority;
use crate::state::ServerState;

/// The reads per asset name in the caller's tenant and graph. A deployment without a series
/// store, or a caller without verified tenant authority, has none.
#[cfg(feature = "tsdb")]
pub async fn load_reads_index(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    carrier: Option<&CarrierAuthority>,
) -> Option<Arc<UsageIndex>> {
    use medulla_catalog_types::asset_usage::ASSET_USAGE_READS_SERIES;
    use medulla_tsdb::store::SeriesKey;

    let carrier = carrier?;
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
    Some(Arc::new(
        medulla_catalog_types::catalog_relevance::usage_index(&series),
    ))
}

/// Without the series build no usage is recorded, so the index is empty.
#[cfg(not(feature = "tsdb"))]
pub async fn load_reads_index(
    _state: &Arc<RwLock<ServerState>>,
    _graph_name: &str,
    _carrier: Option<&CarrierAuthority>,
) -> Option<Arc<UsageIndex>> {
    None
}
