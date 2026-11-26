use super::*;

use super::terminal::GraphOpsContext;

/// Every edge's endpoints paired with its decoded property blob (`Null` for
/// an undecodable one). The subgraph and cluster-hierarchy wire projections
/// both start by walking `GraphView::edge_properties` and decoding each blob
/// exactly this way — one walk so a fix to how a bad blob decodes can't land
/// on one wire shape and not the other.
pub(super) fn decoded_edge_properties(
    sub: &medulla_graph_core::graph::GraphView,
) -> Vec<(String, String, serde_json::Value)> {
    let mut decoded = Vec::new();
    for ((src, tgt), blobs) in &sub.edge_properties {
        for blob in blobs {
            let props = medulla_types::msgpack::decode_property_value(blob)
                .unwrap_or(serde_json::Value::Null);
            decoded.push((src.clone(), tgt.clone(), props));
        }
    }
    decoded
}

/// `GetSubgraph`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_get_subgraph(req_id: u64, core: &Arc<GraphCore>, node_ids: &[String]) -> Response {
    // Batched subgraph read: return the induced nodes (with DECODED
    // properties) and the edges among them in ONE round-trip, so callers
    // never loop per-node `GetNodeProperties` or pull the whole edge set.
    // (Previously serialized to msgpack then mis-parsed as JSON → error.)
    let g = &**core;
    let sub = g.get_subgraph(node_ids);
    let mut nodes = Vec::with_capacity(sub.node_properties.len());
    for (id, blob) in &sub.node_properties {
        let props =
            medulla_types::msgpack::decode_property_value(blob).unwrap_or(serde_json::Value::Null);
        nodes.push(medulla_types::types::SubgraphNode::new(id.clone(), props));
    }
    let edges: Vec<_> = decoded_edge_properties(&sub)
        .into_iter()
        .map(|(source, target, properties)| {
            medulla_types::types::SubgraphEdge::new(source, target, properties)
        })
        .collect();
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::graph::GetSubgraph>(
            medulla_types::types::SubgraphResult { nodes, edges },
        ),
    )
}

fn neighborhood_refusal(code: medulla_types::contract::ServerErrorCode, detail: &str) -> String {
    format!("{}: {detail}", code.as_str())
}

/// Whether an edge property blob names a relationship the walk follows.
fn blob_follows(blob: &[u8], edge_types: &[String]) -> bool {
    if edge_types.is_empty() {
        return true;
    }
    let rels = medulla_graph_core::index::EdgeRels::from_blobs([blob]);
    edge_types.iter().any(|kind| rels.types.contains(kind))
}

/// `GetNeighborhood` (MEDULLA-DURABLE-KERNEL-R876.3): one bounded breadth-first read over the
/// caller's projected core, so a node the caller may not read is `NODE_NOT_FOUND` and a hidden
/// count never includes a neighbor the caller may not read.
fn handle_get_neighborhood(
    req_id: u64,
    core: &Arc<GraphCore>,
    raw_core: &Arc<GraphCore>,
    request: NeighborhoodRequest<'_>,
) -> Response {
    use medulla_graph_core::neighborhood::{self, NeighborhoodQuery};
    use medulla_types::contract::ServerErrorCode;
    let graph_revision = raw_core.version();
    let skip = match request.cursor {
        None => 0,
        Some(cursor) => match neighborhood::decode_cursor(cursor) {
            Some((revision, skip)) if revision == graph_revision => skip,
            _ => {
                return Response::err(
                    req_id,
                    neighborhood_refusal(
                        ServerErrorCode::NeighborhoodCursorStale,
                        "the graph revision moved since the cursor was issued",
                    ),
                )
            }
        },
    };
    let query = NeighborhoodQuery {
        root: request.node_id,
        depth: request.depth,
        edge_types: request.edge_types,
        per_node_limit: request.per_node_limit,
        total_limit: request.total_limit,
        exclude: request.exclude,
        skip,
    };
    let walked = match neighborhood::walk(&**core, &query) {
        Ok(walked) => walked,
        Err(refusal) => {
            return Response::err(
                req_id,
                neighborhood_refusal(refusal.code(), "the neighborhood read was refused"),
            )
        }
    };
    let nodes: Vec<_> = walked
        .nodes
        .iter()
        .map(|id| {
            medulla_types::types::SubgraphNode::new(
                id.clone(),
                core.get_node_properties(id)
                    .and_then(|blob| medulla_types::msgpack::decode_property_value(&blob).ok())
                    .unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let mut edges = Vec::with_capacity(walked.edges.len());
    for edge in &walked.edges {
        let Some(blobs) = core
            .edge_properties
            .get(&(edge.source.clone(), edge.target.clone()))
        else {
            continue;
        };
        for blob in blobs.iter().filter(|b| blob_follows(b, request.edge_types)) {
            edges.push(medulla_types::types::SubgraphEdge::new(
                edge.source.clone(),
                edge.target.clone(),
                medulla_types::msgpack::decode_property_value(blob)
                    .unwrap_or(serde_json::Value::Null),
            ));
        }
    }
    let result = medulla_types::types::NeighborhoodResult {
        nodes,
        edges,
        hidden: walked
            .hidden
            .iter()
            .map(|(node_id, count)| medulla_types::types::HiddenCount {
                node_id: node_id.clone(),
                count: *count,
            })
            .collect(),
        next_cursor: walked
            .next_skip
            .map(|next| neighborhood::encode_cursor(graph_revision, next)),
        truncated: walked.truncated,
        graph_revision,
    };
    tracing::info!(
        target: "medulla::read_audit",
        "{}",
        neighborhood::audit_line(
            request.node_id,
            request.depth,
            result.nodes.len(),
            result.edges.len(),
            result.truncated,
        )
    );
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::graph::GetNeighborhood>(
            result,
        ),
    )
}

/// The borrowed arguments of one `GetNeighborhood`.
struct NeighborhoodRequest<'a> {
    node_id: &'a str,
    depth: u32,
    edge_types: &'a [String],
    per_node_limit: u32,
    total_limit: u32,
    exclude: &'a [String],
    cursor: Option<&'a str>,
}

fn prepare_fork_value(
    snapshot: &medulla_graph_core::graph::GraphSnapshot,
) -> Result<serde_json::Value, String> {
    let mut value = snapshot.to_msgpack().and_then(|bytes| {
        rmp_serde::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())
    })?;
    let nodes = decode_fork_nodes(&snapshot.nodes)?;
    let edges = decode_fork_edges(&snapshot.edges)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "fork snapshot did not encode as a JSON object".to_string())?;
    object.insert("nodes".to_string(), serde_json::Value::Array(nodes));
    object.insert("edges".to_string(), serde_json::Value::Array(edges));
    Ok(value)
}

/// `Fork`: pure extract-method from `try_handle`'s match arm, byte-identical
/// behaviour, no signature change.
fn handle_fork(req_id: u64, core: &Arc<GraphCore>) -> Response {
    // Cannot return the forked GraphCore directly because it needs to be registered.
    // A true fork method in the registry might be better.
    // Return the snapshot's established JSON shape, replacing its internal property blobs
    // with the caller-authored property values promised by the Fork result contract.
    let snapshot = (**core).fork().snapshot();
    let value = match prepare_fork_value(&snapshot) {
        Ok(value) => value,
        Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
    };

    Response::ok(
        req_id,
        ResultPayload::of_dynamic::<medulla_protocol_types::result_contract::graph::Fork, _>(
            &value,
        ),
    )
}

fn decode_fork_property(blob: &[u8], subject: &str) -> Result<serde_json::Value, String> {
    medulla_types::msgpack::decode_property_value(blob)
        .map_err(|_| format!("fork {subject} has an invalid property blob"))
}

fn decode_fork_nodes(nodes: &[(String, Arc<Vec<u8>>)]) -> Result<Vec<serde_json::Value>, String> {
    nodes
        .iter()
        .map(|(id, blob)| {
            decode_fork_property(blob, &format!("node '{id}'")).map(|properties| {
                serde_json::Value::Array(vec![serde_json::Value::String(id.clone()), properties])
            })
        })
        .collect()
}

fn decode_fork_edges(
    edges: &[(String, String, Arc<Vec<u8>>)],
) -> Result<Vec<serde_json::Value>, String> {
    edges
        .iter()
        .map(|(source, target, blob)| {
            decode_fork_property(blob, &format!("edge '{source}' -> '{target}'")).map(
                |properties| {
                    serde_json::Value::Array(vec![
                        serde_json::Value::String(source.clone()),
                        serde_json::Value::String(target.clone()),
                        properties,
                    ])
                },
            )
        })
        .collect()
}

/// `Vf2SubgraphMatch`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
pub(super) async fn handle_vf2_subgraph_match(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    read_authority: &GraphReadAuthority,
    core: &Arc<GraphCore>,
    pattern_graph_name: String,
    max_results: usize,
    max_steps: usize,
) -> Response {
    let s = state.read().await;
    // The pattern graph is read too — gate it like any other read.
    if let Some(entry) = s.registry.get(&pattern_graph_name) {
        if let Err(denied) = check_graph_access(
            &s.isolation,
            read_authority.actor(),
            &pattern_graph_name,
            entry.graph_type,
            entry.owner.as_deref(),
            AccessLevel::Read,
        ) {
            return Response::err(req_id, denied);
        }
    }
    let pattern_core = s
        .registry
        .get(&pattern_graph_name)
        .map(|entry| entry.core.clone());
    drop(s);
    if let Some(p_core) = pattern_core {
        // Exponential-worst-case matching never runs under either
        // graph's lock: snapshot pattern then host SEQUENTIALLY (no
        // nested cross-graph locks), compute off-lock (KG-2.51).
        let p_core = read_authority.project_core(&p_core);
        let p_snap = p_core.analysis_snapshot();
        // vf2_subgraph_match snapshots the host internally, so the
        // NP-hard backtracking (bounded by max_results/max_steps) runs
        // entirely off-lock.
        let host = core.clone();
        match compute_off_lock(req_id, move || {
            host.vf2_subgraph_match(&p_snap, max_results, max_steps)
        })
        .await
        {
            Ok((matches, truncated)) => Response::ok(
                req_id,
                ResultPayload::of::<
                    medulla_protocol_types::result_contract::compute::Vf2SubgraphMatch,
                >(
                    medulla_result_types::compute_result::algorithms::Vf2MatchResult {
                        matches,
                        truncated,
                    },
                ),
            ),
            Err(resp) => resp,
        }
    } else {
        Response::err(
            req_id,
            format!("Pattern graph '{}' not found", pattern_graph_name),
        )
    }
}

/// Handle subgraph extraction and forking.
pub(super) async fn try_handle_subgraph_reads(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext {
        req_id,
        core,
        raw_core,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        Method::GetSubgraph { node_ids } => handle_get_subgraph(req_id, core, &node_ids),
        Method::GetNeighborhood {
            node_id,
            depth,
            edge_types,
            per_node_limit,
            total_limit,
            exclude,
            cursor,
        } => handle_get_neighborhood(
            req_id,
            core,
            raw_core,
            NeighborhoodRequest {
                node_id: &node_id,
                depth,
                edge_types: &edge_types,
                per_node_limit,
                total_limit,
                exclude: &exclude,
                cursor: cursor.as_deref(),
            },
        ),
        Method::Fork => handle_fork(req_id, core),
        other => return ControlFlow::Continue(other),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn fork_snapshot_projects_decoded_nested_properties() {
        use medulla_graph_core::graph::GraphCore;
        use medulla_protocol_types::protocol::ResultPayload;
        use std::sync::Arc;

        let core = Arc::new(GraphCore::new());
        core.add_node(
            "n1".to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Doc",
                "metadata": {"priority": 7, "tags": ["a", "b"]}
            }))
            .expect("encode test properties"),
        );
        core.add_node(
            "n2".to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({"type": "Folder"}))
                .expect("encode test properties"),
        );
        core.add_edge(
            "n1".to_string(),
            "n2".to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "relationship": "CONTAINS",
                "metadata": {"weight": 0.75}
            }))
            .expect("encode test edge properties"),
        )
        .expect("add test edge");

        let response = super::handle_fork(1, &core);
        assert!(
            response.error.is_none(),
            "fork failed: {:?}",
            response.error
        );
        let ResultPayload::Json(snapshot) = response.result.expect("fork result") else {
            panic!("fork result was not JSON");
        };
        let object = snapshot.as_object().expect("fork snapshot object");
        let node = object["nodes"]
            .as_array()
            .expect("fork nodes")
            .iter()
            .find(|row| row[0] == "n1")
            .expect("n1 row");
        assert_eq!(
            node[1],
            serde_json::json!({
                "type": "Doc",
                "metadata": {"priority": 7, "tags": ["a", "b"]}
            })
        );
        let edge = object["edges"]
            .as_array()
            .expect("fork edges")
            .iter()
            .find(|row| row[0] == "n1" && row[1] == "n2")
            .expect("n1 -> n2 row");
        assert_eq!(
            edge[2],
            serde_json::json!({
                "relationship": "CONTAINS",
                "metadata": {"weight": 0.75}
            })
        );
    }

    #[test]
    fn fork_snapshot_rejects_malformed_property_blob() {
        use medulla_graph_core::graph::GraphCore;
        use std::sync::Arc;

        let core = Arc::new(GraphCore::new());
        core.add_node("bad".to_string(), vec![0xc1]);

        let response = super::handle_fork(2, &core);

        assert!(response.result.is_none());
        assert_eq!(
            response.error_detail.as_deref(),
            Some("fork node 'bad' has an invalid property blob")
        );
    }

    #[test]
    fn fork_snapshot_rejects_malformed_edge_property_blob() {
        use medulla_graph_core::graph::GraphCore;
        use std::sync::Arc;

        let core = Arc::new(GraphCore::new());
        let properties = rmp_serde::to_vec_named(&serde_json::json!({"type": "Node"}))
            .expect("encode test properties");
        core.add_node("n1".to_string(), properties.clone());
        core.add_node("n2".to_string(), properties);
        core.add_edge("n1".to_string(), "n2".to_string(), vec![0xc1])
            .expect("add test edge");

        let response = super::handle_fork(3, &core);

        assert!(response.result.is_none());
        assert_eq!(
            response.error_detail.as_deref(),
            Some("fork edge 'n1' -> 'n2' has an invalid property blob")
        );
    }
}
