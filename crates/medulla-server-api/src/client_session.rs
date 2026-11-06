//! MEDULLA-IDENTITY-R705 -- admission of `ClientSession` records.
//!
//! A `ClientSession` node is written by a client (an MCP session open) through
//! the served node write. The node id is `client_session:<tenant>:<session_id>`
//! and its `principal` property must be the verified caller, unless the caller
//! holds `identity:admin`. A node whose id names another tenant is refused with
//! the same code, so a session never lands in a tenant its writer is not in.

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

fn is_session_label(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props.get("type").and_then(serde_json::Value::as_str) == Some(CLIENT_SESSION_LABEL)
}

fn names_caller(
    node_id: &str,
    props: &serde_json::Map<String, serde_json::Value>,
    context: &VerifiedRequestContext,
) -> bool {
    let principal = props.get("principal").and_then(serde_json::Value::as_str);
    let tenant_prefix = client_session_node_id(context.tenant(), "");
    principal == Some(context.principal()) && node_id.starts_with(&tenant_prefix)
}

/// Admit or refuse a node write at the gateway. Every write that is not a
/// `ClientSession` record passes untouched.
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
    if !is_session_label(&props)
        || context.allows_action(IDENTITY_ADMIN_SCOPE)
        || names_caller(node_id, &props, context)
    {
        return Ok(());
    }
    Err(ServerErrorCode::ClientSessionPrincipalMismatch)
}
