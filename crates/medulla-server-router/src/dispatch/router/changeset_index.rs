//! The router side of the code-graph re-index of an accepted changeset
//! (MEDULLA-SCM-R006): read the stored index of the repository graph, call
//! `reindex_changeset`, and commit its write-set through the ordinary graph
//! gateway as one `BatchUpdate`, so the write is ACL-checked, sealed and durable
//! before the receipt returns.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::repository_index::changeset::{
    generation_node_id, reindex_changeset, ChangesetFile, StoredIndex,
};
use medulla_h_ingest::repository_index::delta::RepositoryGeneration;
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

/// The most nodes of one label the stored-index read scans.
const MAX_SCAN: usize = 10_000_000;

fn decode(blob: &[u8]) -> Option<Value> {
    medulla_types::msgpack::decode_property_value(blob).ok()
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The stored index of `repo` as `reindex_changeset` reads it: the generation, the
/// fingerprints of the changed paths, the nodes of those paths, and the resolved
/// `calls` and community labels of the call components that hold them.
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
        .map(|node| RepositoryGeneration {
            map_generation: text(&node, "map_generation")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            base_revision: text(&node, "base_revision").unwrap_or_default().to_string(),
            complete: text(&node, "complete") == Some("true"),
        });
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
    for label in ["FILE", "SYMBOL"] {
        for (id, blob) in core.get_nodes_by_label(label, MAX_SCAN) {
            let Some(node) = decode(&blob) else { continue };
            if let Some(path) = text(&node, "file_path").filter(|path| paths.contains(path)) {
                nodes_by_path.entry(path.to_string()).or_default().push(id);
            }
        }
    }
    let mut adjacent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut calls: Vec<(String, String)> = Vec::new();
    let mut feature_of: BTreeMap<String, String> = BTreeMap::new();
    for (source, target, blob) in core.get_edges_arc() {
        let Some(edge) = decode(&blob) else { continue };
        match text(&edge, "relationship") {
            Some("calls") => {
                adjacent
                    .entry(source.clone())
                    .or_default()
                    .push(target.clone());
                adjacent
                    .entry(target.clone())
                    .or_default()
                    .push(source.clone());
                calls.push((source, target));
            }
            Some("memberOfFeature") => {
                if let Some(label) = target.strip_prefix("feature:") {
                    feature_of.insert(source, label.to_string());
                }
            }
            _ => {}
        }
    }
    let mut component: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for id in nodes_by_path.values().flatten() {
        if component.insert(id.clone()) {
            queue.push_back(id.clone());
        }
    }
    while let Some(node) = queue.pop_front() {
        for next in adjacent.get(&node).into_iter().flatten() {
            if component.insert(next.clone()) {
                queue.push_back(next.clone());
            }
        }
    }
    Ok(StoredIndex {
        generation,
        prints,
        nodes_by_path,
        calls: calls
            .into_iter()
            .filter(|(source, _)| component.contains(source))
            .collect(),
        communities: component
            .iter()
            .filter_map(|id| feature_of.get(id).map(|label| (id.clone(), label.clone())))
            .collect(),
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
        let outcome = reindex_changeset(&changeset, &commit_id, &files, &stored)?;
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
