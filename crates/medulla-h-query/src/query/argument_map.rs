//! Argument maps with a Dung projection (MEDULLA-FEDERATED-QUERY-R700.3):
//! `ArgumentMapGet` and `ArgumentMapEvaluate`.
//!
//! The write half (`ArgumentMapPut`) is GATEWAY_ROUTED and lands in
//! `medulla_h_graph::graph_ops::argument_map`; this module is read-only. It
//! re-reads the stored interchange document from the `ArgumentMap` node and
//! evaluates its Dung projection: `grounded` is the fixed-point iteration,
//! `preferred` enumerates under the caller's `node_budget` or the
//! `argument.node_budget` knob. Evaluation is a read and stores nothing.

use medulla_protocol_types::result_contract::query as results;
use medulla_result_types::argument_map::{ArgumentMap, ExtensionResult};
use medulla_server_api::knob_config::knobs;
use medulla_types::msgpack::decode_property_object;
use serde_json::Value;

use super::*;

const NOT_FOUND: &str = "ARGUMENT_MAP_NOT_FOUND";
const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";

pub(crate) fn is_argument_map_method(method: &Method) -> bool {
    matches!(
        method,
        Method::ArgumentMapGet { .. } | Method::ArgumentMapEvaluate { .. }
    )
}

fn map_node(map_id: &str) -> String {
    format!("argument_map:{map_id}")
}

fn reply<M: MethodResult>(req_id: u64, out: Result<M::Body, String>) -> Response {
    match out.and_then(ResultPayload::of::<M>) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// Route one argument-map method to its handler.
pub(crate) async fn dispatch_argument_map(
    ctx: &QueryHandlerCtx<'_>,
    method: Method,
) -> Result<Response, Method> {
    let req_id = ctx.req_id;
    match method {
        Method::ArgumentMapGet { map_id } => Ok(reply::<results::ArgumentMapGet>(
            req_id,
            handle_get(ctx, &map_id),
        )),
        Method::ArgumentMapEvaluate {
            map_id,
            semantics,
            node_budget,
        } => Ok(reply::<results::ArgumentMapEvaluate>(
            req_id,
            handle_evaluate(ctx, &map_id, &semantics, node_budget),
        )),
        other => Err(other),
    }
}

/// The stored interchange document of one map, parsed back from the
/// canonical JSON the write committed.
fn stored_map(ctx: &QueryHandlerCtx<'_>, map_id: &str) -> Result<ArgumentMap, String> {
    let blob = ctx
        .core
        .get_node_properties(&map_node(map_id))
        .ok_or_else(|| format!("{NOT_FOUND}: no argument map `{map_id}`"))?;
    let props = decode_property_object(&blob)
        .map_err(|_| format!("{INVALID_ARGUMENT}: stored map node is not a property object"))?;
    let json = props
        .get("json")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{INVALID_ARGUMENT}: stored map node has no document"))?;
    serde_json::from_str(json).map_err(|error| format!("{INVALID_ARGUMENT}: {error}"))
}

fn handle_get(ctx: &QueryHandlerCtx<'_>, map_id: &str) -> Result<ArgumentMap, String> {
    stored_map(ctx, map_id)
}

fn handle_evaluate(
    ctx: &QueryHandlerCtx<'_>,
    map_id: &str,
    semantics: &str,
    node_budget: Option<u64>,
) -> Result<ExtensionResult, String> {
    let map = stored_map(ctx, map_id)?;
    let map_digest = map.digest().map_err(|refusal| refusal.to_string())?;
    let projection = map.project().map_err(|refusal| refusal.to_string())?;
    let (extensions, complete, nodes_used) = match semantics {
        "grounded" => (vec![projection.grounded()], true, 0),
        "preferred" => {
            let found = projection
                .preferred(node_budget.unwrap_or_else(|| knobs().argument_node_budget));
            (found.extensions, found.complete, found.nodes_explored)
        }
        other => {
            return Err(format!(
                "ARGUMENT_MAP_INVALID_SEMANTICS: `semantics` is `grounded` or `preferred`, got `{other}`"
            ))
        }
    };
    Ok(ExtensionResult {
        semantics: semantics.to_string(),
        extensions,
        complete,
        nodes_used,
        map_digest,
    })
}
