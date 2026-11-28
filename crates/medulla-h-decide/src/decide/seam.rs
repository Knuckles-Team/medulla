//! The Decide handler's implementation of the server's `DecideApi` seam. It names the served
//! adapter and the decision views, so it sits above both.

/// The caller's read-only SQL relations — the decision record views (EH-066) — when
/// `query` can see them; `None` otherwise, or in a build without the Decide layer.
#[cfg(feature = "query")]
async fn read_only_relations(
    state: &medulla_h_decide_base::decide::SharedState,
    authority: &medulla_server_api::carrier::CarrierAuthority,
    query: &str,
) -> Option<medulla_server_auth::sql_catalog_acl::SharedRelations> {
    #[cfg(feature = "decide")]
    {
        if !medulla_server_auth::sql_catalog_acl::wants_read_only_relations(query) {
            return None;
        }
        let views =
            medulla_h_decide_learning::decide::stat_view::DecisionViews::served(state, authority)
                .await?;
        Some(views as medulla_server_auth::sql_catalog_acl::SharedRelations)
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, authority, query);
        None
    }
}

/// The caller's visible decision log as a UQL `DECISIONS` source (EH-066).
#[cfg(all(feature = "decide", feature = "query"))]
async fn decision_source(
    state: &medulla_h_decide_base::decide::SharedState,
    authority: &medulla_server_api::carrier::CarrierAuthority,
) -> Option<std::sync::Arc<dyn medulla_exec::exec::DecisionSource>> {
    let views =
        medulla_h_decide_learning::decide::stat_view::DecisionViews::served(state, authority)
            .await?;
    Some(views as std::sync::Arc<dyn medulla_exec::exec::DecisionSource>)
}

/// The decide handler's implementation of [`DecideApi`](medulla_server::api::admin::DecideApi).
pub struct DecideSeam;

#[async_trait::async_trait]
impl medulla_server_ops::api::admin::DecideApi for DecideSeam {
    #[cfg(feature = "decide")]
    async fn served_adapter(
        &self,
        state: &medulla_h_decide_base::decide::SharedState,
        tenant: Option<&str>,
        graph: &str,
        core: &medulla_graph_core::graph::GraphCore,
        capability: Option<&str>,
    ) -> Option<std::sync::Arc<medulla_server_ops::api::admin::served_adapter::ServedAdapter>> {
        medulla_h_decide_base::decide::served_adapter::served_for(
            state, tenant, graph, core, capability,
        )
        .await
    }

    #[cfg(feature = "query")]
    async fn read_only_relations(
        &self,
        state: &medulla_h_decide_base::decide::SharedState,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        query: &str,
    ) -> Option<medulla_server_auth::sql_catalog_acl::SharedRelations> {
        read_only_relations(state, authority, query).await
    }

    #[cfg(all(feature = "decide", feature = "query"))]
    async fn decision_source(
        &self,
        state: &medulla_h_decide_base::decide::SharedState,
        authority: &medulla_server_api::carrier::CarrierAuthority,
    ) -> Option<std::sync::Arc<dyn medulla_exec::exec::DecisionSource>> {
        decision_source(state, authority).await
    }
}
