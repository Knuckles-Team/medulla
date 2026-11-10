//! Served argument-map writes (MEDULLA-FEDERATED-QUERY-R700.3).
//!
//! `ArgumentMapPut` is gateway-routed (`Route::ArgumentMap`): the map is
//! validated by `ArgumentMap::checked` (a refused map writes nothing), then
//! commits as one atomic batch through the staged gateway — the `ArgumentMap`
//! node holding the canonical interchange JSON, one `AifNode` node per map
//! node (properties `aif_id`, `kind`, `text`) and one `AIF_EDGE`
//! relationship per map edge (property `edge_id`). Replacing a stored map
//! drops its prior projection first. `ArgumentMapGet` and
//! `ArgumentMapEvaluate` are query-family reads served by `medulla-h-query`
//! and never reach this module.

use super::*;
use medulla_protocol_types::result_contract::query::ArgumentMapPut;
use medulla_result_types::argument_map::ArgumentMap;
use serde_json::json;

const MAP_LABEL: &str = "ArgumentMap";
const NODE_LABEL: &str = "AifNode";
const EDGE_LABEL: &str = "AIF_EDGE";

fn map_node(map_id: &str) -> String {
    format!("argument_map:{map_id}")
}

fn aif_node(map_id: &str, aif_id: &str) -> String {
    format!("aif_node:{map_id}:{aif_id}")
}

fn msgpack(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("AIF_MALFORMED: property encoding failed: {error}"))
}

/// The stored interchange document of one map, if present and readable.
fn read_stored_map(core: &GraphCore, map_id: &str) -> Option<ArgumentMap> {
    let blob = core.get_node_properties(&map_node(map_id))?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    let json_text = props
        .get("json")
        .and_then(|value| value.as_str())?
        .to_string();
    serde_json::from_str(&json_text).ok()
}

/// The `Route::ArgumentMap` entry: validate, then commit the one atomic batch.
pub(super) async fn handle_put(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ArgumentMapPut { map_id, map } = method else {
        return None;
    };
    let _ = tenant_id;
    if map_id.trim().is_empty() {
        return Some(Response::err(ctx.req_id, "AIF_MALFORMED: empty map_id"));
    }
    let map = match map.clone().checked() {
        Ok(map) => map,
        Err(refusal) => return Some(Response::err(ctx.req_id, refusal.to_string())),
    };
    let json = match serde_json::to_string(&map) {
        Ok(json) => json,
        Err(error) => {
            return Some(Response::err(
                ctx.req_id,
                format!("AIF_MALFORMED: map encoding failed: {error}"),
            ))
        }
    };
    let previous = read_stored_map(ctx.core, map_id);
    let plan = MutationPlan::for_method(method);
    let owned_id = map_id.clone();
    let owned_map = map;
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, method, move |core| {
            // Replace: drop the prior projection (node removal clears its
            // `AIF_EDGE` relationships), then write the new batch.
            if let Some(previous) = previous {
                for node in &previous.nodes {
                    core.remove_node(aif_node(&owned_id, &node.node_id));
                }
            }
            core.remove_node(map_node(&owned_id));
            let map_props = json!({
                "type": MAP_LABEL,
                "map_id": owned_id,
                "json": json,
            });
            let _ = core.add_node(map_node(&owned_id), msgpack(&map_props)?);
            for node in &owned_map.nodes {
                let props = json!({
                    "type": NODE_LABEL,
                    "aif_id": node.node_id,
                    "kind": node.kind,
                    "text": node.text,
                });
                let _ = core.add_node(aif_node(&owned_id, &node.node_id), msgpack(&props)?);
            }
            for edge in &owned_map.edges {
                let props = json!({
                    "relationship": EDGE_LABEL,
                    "edge_id": edge.edge_id,
                });
                core.add_edge(
                    aif_node(&owned_id, &edge.from_id),
                    aif_node(&owned_id, &edge.to_id),
                    msgpack(&props)?,
                )
                .map_err(|error| format!("AIF_MALFORMED: edge write failed: {error}"))?;
            }
            ResultPayload::of::<ArgumentMapPut>(owned_map)
        })
        .await,
    )
}
