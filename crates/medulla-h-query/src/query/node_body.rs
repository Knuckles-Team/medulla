//! `Method::NodeBody` (MEDULLA-INGEST-WORKER-R003.21): the stored body text of one node at a
//! revision, over the caller's row-projected core.
//!
//! The handler reads the node out of the projection and calls the one `answer_for` of
//! `medulla_types::node_body`. A node the caller cannot read is not in the projected core, so
//! the read never reaches it and answers exactly like an absent node. Nothing here writes.

use super::*;
use medulla_protocol_types::result_contract::coordination::NodeBody;
use medulla_types::node_body::{answer_for, NodeBodyRequest, NODE_BODY_UNAVAILABLE};

/// Serve one `NodeBody` request over the caller's projection of the request graph.
pub(crate) fn handle_node_body(ctx: &QueryHandlerCtx<'_>, request: NodeBodyRequest) -> Response {
    let Some((authority, carrier)) = ctx
        .read_authority
        .and_then(|authority| authority.carrier().map(|carrier| (authority, carrier)))
    else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: a node body read needs a verified caller",
        );
    };
    if let Err(error) = request.validate() {
        return Response::err(ctx.req_id, error);
    }
    // A tenant other than the verified one is refused alike whether or not it exists.
    if medulla_server_api::carrier::tenant_scope_of(&request.tenant) != carrier.tenant_scope() {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: node body tenant must match the verified request tenant",
        );
    }
    let projected = authority.project_core(ctx.core);
    let Some(blob) = projected.get_node_properties(&request.node_id) else {
        return Response::err(ctx.req_id, NODE_BODY_UNAVAILABLE);
    };
    match answer_for(&request.node_id, &blob, request.revision) {
        Ok(answer) => Response::ok(ctx.req_id, ResultPayload::of::<NodeBody>(answer)),
        Err(code) => Response::err(ctx.req_id, code),
    }
}
