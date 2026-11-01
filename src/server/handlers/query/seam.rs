//! `QueryApi` implementation: query behaviour that crates below the query handler reach
//! through the seam registry (MEDULLA-CONTRACT-R056.8).

#[allow(unused_imports)]
use std::sync::Arc;

/// Query handler behaviour installed on the seam registry's `query` slot.
pub struct QuerySeam;

#[async_trait::async_trait]
impl medulla_server_api::api::query_txn::QueryApi for QuerySeam {
    #[cfg(feature = "ann-redb")]
    fn existing_semantic_service(
        &self,
        persist_dir: &std::path::Path,
        tenant: &str,
        binding_id: &str,
    ) -> Result<Arc<medulla_ann_codes::semantic_index_service::SemanticIndexService>, String> {
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
        tsdb_ctx: medulla_server_api::api::query_txn::query_support::TsdbLegBind<'_>,
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
            medulla_server_api::api::query_txn::query_support::execute_rows,
        )
    }

    #[cfg(feature = "mining")]
    async fn mining_gateway_try_handle(
        &self,
        ctx: &medulla_server_api::mutation::MutationCtx<'_>,
        plan: &medulla_server_api::mutation::MutationPlan,
        method: &medulla_protocol_types::protocol::Method,
        graph_name: &str,
        read_authority: Option<&medulla_server_api::access::GraphReadAuthority>,
        #[cfg(all(feature = "query", feature = "tsdb"))] tsdb_store: Option<
            &Arc<medulla_tsdb::store::SeriesStore>,
        >,
    ) -> Option<medulla_protocol_types::protocol::Response> {
        crate::server::handlers::mining::gateway::try_handle(
            ctx,
            plan,
            method,
            graph_name,
            read_authority,
            #[cfg(all(feature = "query", feature = "tsdb"))]
            tsdb_store,
        )
        .await
    }
}
