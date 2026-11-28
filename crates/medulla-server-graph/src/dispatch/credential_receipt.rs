//! `Method::CredentialReceipt`: the served credential use receipt
//! (MEDULLA-IDENTITY-R753).
//!
//! One `record` writes one `CredentialReceipt` node of the verified tenant through
//! the ordinary graph gateway (`dispatch_graph_op`), so the write is ACL-checked,
//! durable, audited and CDC-emitted. The node id is a digest over the receipt's own
//! fields, so a retried decision stores the same node. A receipt that names another
//! tenant refuses `ACCESS_DENIED` before any write; the receipt carries no secret.

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::CredentialReceipt as CredentialReceiptResult;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;
use {
    medulla_identity_core_types::credential_receipt::CredentialReceiptOp,
    medulla_identity_core_types::credential_receipt::CredentialReceiptRecorded,
    medulla_identity_core_types::credential_receipt::RECEIPT_NODE_TYPE,
};

use super::graph_client::{encode, write, GraphTarget, Properties, TENANT_PROPERTY};
use super::shared::typed_response;

/// Serve one `CredentialReceipt` op.
pub async fn dispatch_credential_receipt(
    state: &Arc<RwLock<ServerState>>,
    target: GraphTarget<'_>,
    op: CredentialReceiptOp,
) -> Response {
    let req_id = target.req_id;
    let result = record(state, &target, op).await;
    typed_response::<CredentialReceiptResult>(req_id, result)
}

async fn record(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    op: CredentialReceiptOp,
) -> Result<CredentialReceiptRecorded, String> {
    let CredentialReceiptOp::Record { receipt } = op;
    receipt.check()?;
    if receipt.tenant != target.verified.tenant() {
        return Err("ACCESS_DENIED: the receipt names another tenant".to_string());
    }
    let receipt_id = receipt.node_id();
    let mut properties = Properties::new();
    properties.insert("type".into(), RECEIPT_NODE_TYPE.into());
    properties.insert(TENANT_PROPERTY.into(), receipt.tenant.clone().into());
    properties.insert("client".into(), receipt.client.into());
    properties.insert("connector".into(), receipt.connector.into());
    properties.insert("resource".into(), receipt.resource.into());
    properties.insert("operation".into(), receipt.operation.into());
    properties.insert("outcome".into(), receipt.outcome.into());
    properties.insert("binding_digest".into(), receipt.binding_digest.into());
    properties.insert("at_ms".into(), receipt.at_ms.to_string().into());
    write(
        state,
        target,
        Method::AddNode {
            node_id: receipt_id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    Ok(CredentialReceiptRecorded { receipt_id })
}
