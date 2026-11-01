//! `TxnApi` implementation: transaction behaviour that crates below the transaction handler
//! reach through the seam registry (MEDULLA-CONTRACT-R056.8).

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::server::ServerState;

/// Transaction handler behaviour installed on the seam registry's `txn` slot.
pub struct TxnSeam;

#[async_trait::async_trait]
impl medulla_server_api::api::query_txn::TxnApi for TxnSeam {
    async fn try_handle(
        &self,
        state: &Arc<RwLock<ServerState>>,
        req_id: u64,
        caller: &str,
        verified_context: &medulla_server_api::auth::VerifiedRequestContext,
        method: crate::protocol::Method,
    ) -> Result<crate::protocol::Response, crate::protocol::Method> {
        super::dispatch::try_handle(state, req_id, caller, verified_context, method).await
    }

    async fn commit_cross_modal_txn_with_nonce(
        &self,
        state: &Arc<RwLock<ServerState>>,
        request_id: u64,
        caller: Option<&str>,
        coordinator_id: &str,
        txn: medulla_server_api::txn::GraphTxnState,
        attempt_nonce: Option<medulla_types::contract::Nonce>,
    ) -> Result<bool, String> {
        super::commit_modal::commit_cross_modal_txn_with_nonce(
            state,
            request_id,
            caller,
            coordinator_id,
            txn,
            attempt_nonce,
        )
        .await
    }

    #[cfg(feature = "graphql")]
    async fn commit_graphql_cross_modal(
        &self,
        ctx: medulla_server_api::api::query_txn::txn_support::GraphQlCrossModalCommit<'_>,
    ) -> Result<bool, String> {
        super::commit_modal::commit_graphql_cross_modal(ctx).await
    }

    #[cfg(feature = "graphql")]
    async fn begin_graphql_lifecycle(
        &self,
        state: &Arc<RwLock<ServerState>>,
        req_id: u64,
        caller: &str,
        authority: &medulla_server_api::access::CarrierAuthority,
        method: &crate::protocol::Method,
    ) -> Result<medulla_server_api::api::query_txn::txn_support::GraphQlLifecycleAdmission, String> {
        use medulla_server_api::api::query_txn::txn_support as api;
        let admission =
            super::receipts::begin_graphql_lifecycle(state, req_id, caller, authority, method)
                .await?;
        Ok(match admission {
            super::receipts::GraphQlLifecycleAdmission::Replayed(result) => {
                api::GraphQlLifecycleAdmission::Replayed(result)
            }
            super::receipts::GraphQlLifecycleAdmission::Execute(receipt) => {
                api::GraphQlLifecycleAdmission::Execute(api::LifecycleToken(receipt))
            }
        })
    }

    #[cfg(feature = "graphql")]
    fn finish_graphql_lifecycle(
        &self,
        token: medulla_server_api::api::query_txn::txn_support::LifecycleToken,
        result: crate::protocol::ResultPayload,
    ) -> Result<crate::protocol::ResultPayload, String> {
        let receipt = token
            .0
            .downcast::<super::receipts::TxnLifecycleReceipt>()
            .map_err(|_| {
                "INTERNAL: transaction lifecycle token has an unexpected type".to_string()
            })?;
        super::receipts::finish_graphql_lifecycle(*receipt, result)
    }

    #[cfg(any(feature = "compute-dist", feature = "matview"))]
    async fn reload_matviews(&self, state: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
        crate::server::handlers::dist_compute::reload_matviews(state).await
    }
}
