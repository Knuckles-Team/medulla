//! `QueryApi` implementation: query behaviour that crates below the query handler reach
//! through the seam registry (MEDULLA-CONTRACT-R056.8).

#[allow(unused_imports)]
use std::sync::Arc;

/// Query handler behaviour installed on the seam registry's `query` slot.
pub struct QuerySeam;

impl crate::server::api::query_txn::QueryApi for QuerySeam {
    #[cfg(feature = "ann-redb")]
    fn existing_semantic_service(
        &self,
        persist_dir: &std::path::Path,
        tenant: &str,
        binding_id: &str,
    ) -> Result<Arc<medulla_core::compute::semantic_index_service::SemanticIndexService>, String>
    {
        crate::server::semantic_index::existing_semantic_service(persist_dir, tenant, binding_id)
    }

    #[cfg(feature = "query")]
    fn run_unified_staged_rows(
        &self,
        plan: medulla_plan::Plan,
        view: &crate::graph::GraphView,
        core: &Arc<crate::graph::GraphCore>,
        staged: &[(String, Vec<f32>)],
        #[cfg(feature = "federation")] foreign: Option<
            &medulla_plan::federation::ForeignSourceRegistry,
        >,
        #[cfg(feature = "tsdb")]
        tsdb_ctx: crate::server::api::query_txn::query_support::TsdbLegBind<'_>,
    ) -> Result<Vec<(String, Option<f32>)>, String> {
        super::overlay::run_unified_with_staged_finish(
            plan,
            view,
            core,
            staged,
            #[cfg(feature = "federation")]
            foreign,
            #[cfg(feature = "tsdb")]
            tsdb_ctx,
            crate::server::api::query_txn::query_support::execute_rows,
        )
    }
}
