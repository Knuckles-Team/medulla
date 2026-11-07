//! MEDULLA-IDENTITY-R705 -- admission of `ClientSession` records.
//!
//! A `ClientSession` node is written by a client (an MCP session open) through
//! the served node write. The node id is `client_session:<tenant>:<session_id>`
//! and its `principal` property must be the verified caller, unless the caller
//! holds `identity:admin`. A node whose id names another tenant is refused with
//! the same code, so a session never lands in a tenant its writer is not in.
//!
//! MEDULLA-IDENTITY-R723 -- a `Workstream` node joins the client sessions of
//! one principal on one workspace digest. Its id is
//! `workstream:<tenant>:<workspace_digest>` and its `principal` must be the
//! verified caller; no scope lifts that rule, because a workstream is always
//! the caller's own.

use medulla_identity_types::identity::IDENTITY_ADMIN_SCOPE;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::ServerErrorCode;

use crate::authority_context::VerifiedRequestContext;

/// The node label (`type` property) of a client session record.
pub const CLIENT_SESSION_LABEL: &str = "ClientSession";

/// The node id of a client session: `client_session:<tenant>:<session_id>`.
pub fn client_session_node_id(tenant: &str, session_id: &str) -> String {
    format!("client_session:{tenant}:{session_id}")
}

/// The node label (`type` property) of a workstream record.
pub const WORKSTREAM_LABEL: &str = "Workstream";

/// The node id of a workstream: `workstream:<tenant>:<workspace_digest>`.
pub fn workstream_node_id(tenant: &str, workspace_digest: &str) -> String {
    format!("workstream:{tenant}:{workspace_digest}")
}

fn node_write_parts(method: &Method) -> Option<(&str, &[u8])> {
    match method {
        Method::AddNode {
            node_id,
            properties_msgpack,
        }
        | Method::CreateNodeIfAbsent {
            node_id,
            properties_msgpack,
        } => Some((node_id, properties_msgpack)),
        _ => None,
    }
}

fn label(props: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    props.get("type").and_then(serde_json::Value::as_str)
}

fn names_caller(
    node_id: &str,
    tenant_prefix: &str,
    props: &serde_json::Map<String, serde_json::Value>,
    context: &VerifiedRequestContext,
) -> bool {
    let principal = props.get("principal").and_then(serde_json::Value::as_str);
    principal == Some(context.principal()) && node_id.starts_with(tenant_prefix)
}

/// Admit or refuse a node write at the gateway. Every write that is not a
/// `ClientSession` or `Workstream` record passes untouched.
pub fn admit_node_write(
    method: &Method,
    context: &VerifiedRequestContext,
) -> Result<(), ServerErrorCode> {
    let Some((node_id, blob)) = node_write_parts(method) else {
        return Ok(());
    };
    let Ok(props) = medulla_types::msgpack::decode_property_object(blob) else {
        return Ok(());
    };
    match label(&props) {
        Some(CLIENT_SESSION_LABEL) => {
            let prefix = client_session_node_id(context.tenant(), "");
            if context.allows_action(IDENTITY_ADMIN_SCOPE)
                || names_caller(node_id, &prefix, &props, context)
            {
                Ok(())
            } else {
                Err(ServerErrorCode::ClientSessionPrincipalMismatch)
            }
        }
        Some(WORKSTREAM_LABEL) => {
            let prefix = workstream_node_id(context.tenant(), "");
            if names_caller(node_id, &prefix, &props, context) {
                Ok(())
            } else {
                Err(ServerErrorCode::WorkstreamPrincipalMismatch)
            }
        }
        _ => Ok(()),
    }
}
