//! The router side of the code-graph re-index of an accepted changeset
//! (MEDULLA-SCM-R006): read the stored index of the repository graph, call
//! `reindex_changeset`, and commit its write-set through the ordinary graph
//! gateway as one `BatchUpdate`, so the write is ACL-checked, sealed and durable
//! before the receipt returns.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::repository_index::changeset::{
    generation_node_id, reindex_changeset, stored_generation, ChangesetFile, StoredEdge,
    StoredIndex, StoredLookup, StoredNode,
};
use medulla_h_ingest::repository_index::enrich::fingerprint::{
    fingerprint_node_id, stored_fingerprint,
};
use medulla_h_ingest::source_changeset::{BlobIo, ChunkStoreBlobs};
use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_blob::blob::store::ChunkStore;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{authorized_graph_read, ensure_graph_resident};
use medulla_source_types::source_changeset::{Changeset, ChangesetIndex, PathOp};
use serde_json::{json, Value};
use tokio::sync::RwLock;

/// The most file nodes of one repository graph the stored-index read lists.
const MAX_FILES: usize = 10_000_000;

fn decode(blob: &[u8]) -> Option<Value> {
    medulla_types::msgpack::decode_property_value(blob).ok()
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn stored_node(core: &GraphCore, id: &str) -> Option<StoredNode> {
    let node = core.get_node_properties(id).and_then(|blob| decode(&blob))?;
    Some(StoredNode {
        id: id.to_string(),
        properties: node
            .as_object()?
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect(),
    })
}

/// Every stored edge between `source` and `target`, with its properties.
fn stored_edges(core: &GraphCore, source: &str, target: &str) -> Vec<StoredEdge> {
    core.get_edge_properties(source, target)
        .iter()
        .filter_map(|blob| decode(blob))
        .filter_map(|edge| {
            let properties = edge
                .as_object()?
                .iter()
                .filter(|(key, _)| key.as_str() != "relationship")
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect();
            Some(StoredEdge {
                source: source.to_string(),
                target: target.to_string(),
                edge_type: text(&edge, "relationship")?.to_string(),
                properties,
            })
        })
        .collect()
}

/// Every edge into or out of `id`.
fn edges_at(core: &GraphCore, id: &str) -> Vec<StoredEdge> {
    let mut edges = Vec::new();
    for target in core.get_successors(id).unwrap_or_default() {
        edges.extend(stored_edges(core, id, &target));
    }
    for source in core.get_predecessors(id).unwrap_or_default() {
        edges.extend(stored_edges(core, &source, id));
    }
    edges
}

/// The reads of the repository graph that depend on what the changed files ask
/// for: each is an index lookup (the property index and the label index), never a
/// scan of the graph.
struct CoreLookup<'a>(&'a GraphCore);

impl StoredLookup for CoreLookup<'_> {
    fn definitions(&self, name: &str) -> Result<Vec<StoredNode>, String> {
        let ids = self
            .0
            .nodes_by_property("name", name)
            .ok_or("INDEX_FAILED: the property index cannot cover the symbol names")?;
        Ok(ids
            .iter()
            .filter_map(|id| stored_node(self.0, id))
            .filter(|node| node.properties.get("type").map(String::as_str) == Some("SYMBOL"))
            .collect())
    }

    fn file_paths(&self) -> Result<Vec<String>, String> {
        Ok(self
            .0
            .get_nodes_by_label("FILE", MAX_FILES)
            .into_iter()
            .filter_map(|(id, _)| id.strip_prefix("file:").map(str::to_string))
            .collect())
    }

    fn similar_candidates(&self, bands: &[(String, String)]) -> Result<Vec<StoredNode>, String> {
        let mut ids = BTreeSet::new();
        for (key, value) in bands {
            ids.extend(
                self.0
                    .nodes_by_property(key, value)
                    .ok_or("INDEX_FAILED: the property index cannot cover the similarity bands")?,
            );
        }
        Ok(self.symbols(ids))
    }

    fn referents(&self, bucket: &str) -> Result<Vec<(StoredNode, String)>, String> {
        let mut referents = Vec::new();
        for source in self.0.get_predecessors(bucket).unwrap_or_default() {
            let Some(node) = stored_node(self.0, &source) else {
                continue;
            };
            for edge in stored_edges(self.0, &source, bucket) {
                referents.push((node.clone(), edge.edge_type));
            }
        }
        Ok(referents)
    }

    fn similar_links(&self, ids: &[String]) -> Result<Vec<StoredEdge>, String> {
        let mut links = Vec::new();
        for id in ids {
            links.extend(
                edges_at(self.0, id)
                    .into_iter()
                    .filter(|edge| edge.edge_type == "similar_to" && &edge.source == id),
            );
        }
        Ok(links)
    }
}

impl CoreLookup<'_> {
    /// The stored `SYMBOL` nodes among `ids`.
    fn symbols(&self, ids: BTreeSet<String>) -> Vec<StoredNode> {
        ids.iter()
            .filter_map(|id| stored_node(self.0, id))
            .filter(|node| node.properties.get("type").map(String::as_str) == Some("SYMBOL"))
            .collect()
    }
}

/// The stored index of `repo` as `reindex_changeset` reads it: the generation, the
/// fingerprints of the changed paths, the nodes of those paths (the property index
/// on `file_path`), every edge that touches them, and the resolved `calls` and
/// community labels of the call components that hold them (walked from those
/// nodes along their own edges).
fn stored_index(core: &GraphCore, changeset: &Changeset) -> Result<StoredIndex, String> {
    let repo = changeset.repo.as_str();
    let paths: BTreeSet<&str> = changeset
        .changed_paths
        .iter()
        .map(|changed| changed.path.as_str())
        .collect();
    let generation = core
        .get_node_properties(&generation_node_id(repo))
        .and_then(|blob| decode(&blob))
        .map(|node| stored_generation(&node));
    let mut prints = BTreeMap::new();
    for path in &paths {
        let node_id = fingerprint_node_id(repo, path)?;
        if let Some(print) = core
            .get_node_properties(&node_id)
            .and_then(|blob| decode(&blob))
            .and_then(|node| stored_fingerprint(&node))
        {
            prints.insert(print.path.clone(), print);
        }
    }
    let mut nodes_by_path: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in &paths {
        let ids = core
            .nodes_by_property("file_path", path)
            .ok_or("INDEX_FAILED: the property index cannot cover file_path")?;
        // Every node kind that carries the path: the file node, its symbols, tests,
        // spans and fingerprint.
        nodes_by_path
            .entry(path.to_string())
            .or_default()
            .extend(ids);
    }
    let held: BTreeSet<&String> = nodes_by_path.values().flatten().collect();
    let mut edges: Vec<StoredEdge> = Vec::new();
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut neighbours: BTreeMap<String, StoredNode> = BTreeMap::new();
    for id in &held {
        for edge in edges_at(core, id) {
            let far = if &edge.source == *id {
                &edge.target
            } else {
                &edge.source
            };
            if !held.contains(far) && !neighbours.contains_key(far) {
                if let Some(node) = stored_node(core, far) {
                    neighbours.insert(far.clone(), node);
                }
            }
            let key = (
                edge.source.clone(),
                edge.target.clone(),
                edge.edge_type.clone(),
            );
            if seen.insert(key) {
                edges.push(edge);
            }
        }
    }
    // The call components that hold those nodes, walked edge by edge.
    let mut component: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for id in &held {
        if component.insert((*id).clone()) {
            queue.push_back((*id).clone());
        }
    }
    let mut calls: BTreeSet<(String, String)> = BTreeSet::new();
    let mut communities: BTreeMap<String, String> = BTreeMap::new();
    while let Some(node) = queue.pop_front() {
        for edge in edges_at(core, &node) {
            match edge.edge_type.as_str() {
                "calls" => {
                    calls.insert((edge.source.clone(), edge.target.clone()));
                    let far = if edge.source == node {
                        edge.target
                    } else {
                        edge.source
                    };
                    if component.insert(far.clone()) {
                        queue.push_back(far);
                    }
                }
                "memberOfFeature" if edge.source == node => {
                    if let Some(label) = edge.target.strip_prefix("feature:") {
                        communities.insert(node.clone(), label.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(StoredIndex {
        generation,
        prints,
        nodes_by_path,
        calls: calls.into_iter().collect(),
        communities,
        edges,
        neighbours,
    })
}

/// The `BatchUpdate` operations of a re-index write-set. A removal of a node the
/// graph does not hold is left out, and an add is an upsert so a node removed and
/// written again in one batch lands.
fn operations(core: &GraphCore, methods: &[Method]) -> Result<Vec<Value>, String> {
    let mut operations = Vec::with_capacity(methods.len());
    for method in methods {
        match method {
            Method::RemoveNode { node_id } => {
                if core.get_node_properties(node_id).is_some() {
                    operations.push(json!({"op": "remove_node", "id": node_id}));
                }
            }
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => operations.push(json!({
                "op": "upsert_node",
                "id": node_id,
                "properties": decode(properties_msgpack).ok_or("re-index node properties do not decode")?,
            })),
            Method::RemoveEdge {
                source_id,
                target_id,
            } => operations.push(json!({"op": "remove_edge", "source": source_id, "target": target_id})),
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => operations.push(json!({
                "op": "upsert_edge",
                "source": source_id,
                "target": target_id,
                "properties": decode(properties_msgpack).ok_or("re-index edge properties do not decode")?,
            })),
            other => return Err(format!("a re-index write-set holds no {other:?}")),
        }
    }
    Ok(operations)
}

fn refused(error: &str) -> ChangesetIndex {
    let code = error.split(':').next().unwrap_or_default().trim();
    let known = !code.is_empty()
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    ChangesetIndex::Refused {
        code: if known { code } else { "INDEX_FAILED" }.to_string(),
    }
}

/// Re-index the code graph of `changeset`, carried by branch commit `commit_id`.
/// `None` when the repository graph is not resident: there is no index to keep.
pub async fn reindex(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    store: Arc<dyn ChunkStore>,
    owner: String,
    changeset: Changeset,
    commit_id: String,
    req_id: u64,
) -> Option<ChangesetIndex> {
    let repo = changeset.repo.clone();
    ensure_graph_resident(state, &repo).await;
    let core = {
        let current = state.read().await;
        authorized_graph_read(&current, &repo, verified, "GRAPH_NOT_FOUND")
            .ok()
            .map(|(core, _)| core)?
    };
    let work_core = Arc::clone(&core);
    let planned = tokio::task::spawn_blocking(move || {
        let blobs = ChunkStoreBlobs(store.as_ref(), &owner);
        let mut files = Vec::new();
        for changed in &changeset.changed_paths {
            if matches!(changed.op, PathOp::Add | PathOp::Modify | PathOp::Rename) {
                let digest = changed
                    .new_blob
                    .as_deref()
                    .ok_or_else(|| format!("CHANGESET_INVALID: {} has no new blob", changed.path))?;
                files.push(ChangesetFile {
                    path: changed.path.clone(),
                    bytes: blobs.read(digest)?,
                });
            }
        }
        let stored = stored_index(&work_core, &changeset)?;
        let outcome = reindex_changeset(&changeset, &commit_id, &files, &stored, &CoreLookup(&work_core))?;
        let operations = operations(&work_core, &outcome.write_set.methods)?;
        Ok::<_, String>((outcome, operations))
    })
    .await;
    let (outcome, operations) = match planned {
        Ok(Ok(planned)) => planned,
        Ok(Err(error)) => return Some(refused(&error)),
        Err(error) => return Some(refused(&error.to_string())),
    };
    let operations_msgpack = match rmp_serde::to_vec_named(&operations) {
        Ok(bytes) => bytes,
        Err(error) => return Some(refused(&error.to_string())),
    };
    let response = dispatch_graph_op(
        state,
        &repo,
        req_id,
        Some(verified.agent_id()),
        verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Some(refused(&refusal));
    }
    Some(ChangesetIndex::Indexed {
        reparsed: outcome.reparsed.len() as u32,
        deleted: outcome.deleted.len() as u32,
        communities: outcome.communities_recomputed as u32,
    })
}
