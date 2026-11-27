//! `QueryApi` implementation: query behaviour that crates below the query handler reach
//! through the seam registry (MEDULLA-CONTRACT-R056.8).

#[allow(unused_imports)]
use std::sync::Arc;

/// Query handler behaviour installed on the seam registry's `query` slot.
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "mining"
))]
pub struct QuerySeam;

#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "mining"
))]
#[async_trait::async_trait]
impl medulla_server_ops::api::query_txn::QueryApi for QuerySeam {
    #[cfg(feature = "federation-sql")]
    fn derive_column_lineage(
        &self,
        sql: &str,
        dialect: medulla_catalog_source_types::lineage::LineageDialect,
        catalog: &medulla_catalog_source_types::attached_catalog::AttachedCatalogGraph,
        target: &str,
    ) -> Result<
        medulla_catalog_source_types::lineage::LineageResult,
        medulla_catalog_source_types::lineage::LineageError,
    > {
        medulla_h_query::query::derive_lineage(sql, dialect, catalog, target)
    }

    #[cfg(feature = "ann-redb")]
    fn existing_semantic_service(
        &self,
        persist_dir: &std::path::Path,
        tenant: &str,
        binding_id: &str,
    ) -> Result<Arc<medulla_ann_codes::semantic_index_service::SemanticIndexService>, String> {
        medulla_h_retrieve::semantic_index_runtime::existing_semantic_service(
            persist_dir,
            tenant,
            binding_id,
        )
    }

    #[cfg(feature = "query")]
    fn run_unified_staged_rows(
        &self,
        plan: medulla_query_types::wire::Plan,
        view: &medulla_graph_core::graph::GraphView,
        core: &Arc<medulla_graph_core::graph::GraphCore>,
        staged: &[(String, Vec<f32>)],
        #[cfg(feature = "federation")] foreign: Option<
            &medulla_federation::federation::ForeignSourceRegistry,
        >,
        #[cfg(feature = "tsdb")]
        tsdb_ctx: medulla_server_ops::api::query_txn::query_support::TsdbLegBind<'_>,
    ) -> Result<Vec<medulla_types::unified_row::UnifiedRow>, String> {
        medulla_h_query::query::overlay::run_unified_with_staged_finish(
            plan,
            view,
            (core, staged),
            #[cfg(feature = "federation")]
            foreign,
            #[cfg(feature = "tsdb")]
            tsdb_ctx,
            None,
            medulla_server_ops::api::query_txn::query_support::execute_rows,
        )
    }

    #[cfg(feature = "query")]
    async fn saved_query_iris(
        &self,
        state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        graph_name: &str,
        read_authority: &medulla_server_api::carrier::GraphReadAuthority,
        caller: &str,
        core: &Arc<medulla_graph_core::graph::GraphCore>,
        text: &str,
        max_rows: usize,
    ) -> Result<Vec<String>, String> {
        #[cfg(feature = "security")]
        let rls = Arc::new(state.read().await.isolation.clone());
        let method = medulla_protocol_types::protocol::Method::Uql {
            text: text.to_string(),
            params: Default::default(),
            prepare: false,
            foreign_executions: Default::default(),
            budget: None,
        };
        let response = medulla_h_query::query::try_handle(
            state,
            medulla_server_api::api::reentry::TryHandleContext {
                req_id: 0,
                graph_name,
                read_authority: Some(read_authority),
                caller,
            },
            core.clone(),
            method,
            #[cfg(feature = "security")]
            &rls,
        )
        .await
        .map_err(|_| "ENGINE_UNAVAILABLE: query surface not available in this build".to_string())?;
        if let Some(error) = response.error {
            return Err(match response.error_detail {
                Some(detail) => format!("{error}: {detail}"),
                None => error,
            });
        }
        let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = response.result
        else {
            return Err("INTERNAL: a saved query answered no rows payload".to_string());
        };
        let result: medulla_query_types::wire::UqlResult = rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("INTERNAL: a saved query answer did not decode: {error}"))?;
        let medulla_query_types::wire::UqlResult::Rows { rows, .. } = result else {
            return Err("INVALID_ARGUMENT: a saved query must return rows".to_string());
        };
        if rows.len() > max_rows {
            return Err(format!(
                "INVALID_ARGUMENT: a saved query returned {} rows, over the limit of {max_rows}",
                rows.len()
            ));
        }
        Ok(rows.into_iter().map(|row| row.id).collect())
    }

    #[cfg(feature = "mining")]
    async fn mining_gateway_try_handle(
        &self,
        ctx: &medulla_server_mutation::mutation::MutationCtx<'_>,
        plan: &medulla_server_mutation::mutation::MutationPlan,
        method: &medulla_protocol_types::protocol::Method,
        graph_name: &str,
        read_authority: Option<&medulla_server_api::carrier::GraphReadAuthority>,
        #[cfg(all(feature = "query", feature = "tsdb"))] tsdb_store: Option<
            &Arc<medulla_tsdb::store::SeriesStore>,
        >,
    ) -> Option<medulla_protocol_types::protocol::Response> {
        medulla_h_mining::mining::gateway::try_handle(
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
