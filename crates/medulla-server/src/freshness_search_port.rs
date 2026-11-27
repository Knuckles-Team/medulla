//! The production [`SearchSourcePort`] of the freshness refresh
//! (MEDULLA-DECISION-ENGINE-R174.9.3.1).
//!
//! `medulla-server` is the one crate that sees both `medulla-federation` and
//! `medulla-h-decide`, so the adapter lives here. The search source of a graph is
//! the foreign source its tenant registered under [`SEARCH_SOURCE_NAME`]; the
//! lookup is tenant-checked and, among several principals of the tenant, takes the
//! lowest owner scope. A source that is not a web metasearch source binds nothing.

use std::sync::Arc;

use medulla_exec::budget::FederationBudget;
use medulla_federation::federation_opt::FederationSession;
use medulla_federation::federation_opt::{web_entry_ttl_ms, web_read};
use medulla_h_decide::decide::stat_freshness::{
    SearchHit, SearchHits, SearchSourceBinding, SearchSourcePort, SEARCH_SOURCE_NAME,
};
use medulla_server_ops::api::query_txn::foreign_catalog::ForeignSourceCatalog;

/// The connection a search binding names: the registered source's own name.
const CONNECTION: &str = SEARCH_SOURCE_NAME;

/// Reads the tenant's registered `search` web source through the federation web path.
pub struct FederationSearchPort {
    catalog: Arc<ForeignSourceCatalog>,
}

impl FederationSearchPort {
    /// A port over the state's foreign source catalog.
    pub fn new(catalog: Arc<ForeignSourceCatalog>) -> Self {
        Self { catalog }
    }
}

impl SearchSourcePort for FederationSearchPort {
    fn binding(&self, tenant: &str, _graph: &str) -> Option<SearchSourceBinding> {
        let spec = self.catalog.spec_for_tenant(tenant, SEARCH_SOURCE_NAME)?;
        // A source that is not a web metasearch source binds nothing.
        web_entry_ttl_ms(&spec, 0)?;
        Some(SearchSourceBinding {
            connection_iri: CONNECTION.to_string(),
            cost_units_per_request: 1,
            tenant: tenant.to_string(),
        })
    }

    fn read(
        &self,
        binding: &SearchSourceBinding,
        query: &str,
        now_ms: u64,
    ) -> Result<SearchHits, String> {
        let unavailable = |detail: &str| format!("SEARCH_SOURCE_UNAVAILABLE: {detail}");
        let spec = self
            .catalog
            .spec_for_tenant(&binding.tenant, SEARCH_SOURCE_NAME)
            .ok_or_else(|| unavailable("no search source is registered for the tenant"))?;
        let session = FederationSession::new(FederationBudget::default());
        let set = web_read(&spec, None, query, &session, now_ms)?
            .ok_or_else(|| unavailable("the registered search source is not a web source"))?;
        Ok(SearchHits {
            query_digest: set.query_digest,
            hits: set
                .candidates
                .into_iter()
                .map(|result| SearchHit {
                    url: result.url,
                    title: result.title,
                    snippet: result.snippet,
                    engine: result.engine,
                    retrieved_at_s: result.retrieved_at,
                })
                .collect(),
        })
    }
}
