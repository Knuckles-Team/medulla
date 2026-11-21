//! The router side of the lifecycle derivation of an accepted changeset
//! (MEDULLA-SDLC-R001.3): plan the changeset node, derive the edges through
//! `accept_changeset_edges`, and apply both as ONE `BatchUpdate` through the ordinary graph
//! gateway, the node ahead of its edges. The write is ACL-checked, sealed and durable before
//! the receipt returns. A repository graph that is not resident has no lifecycle to keep.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_core::lifecycle::accept::{accept_changeset_edges, changed_span, ChangesetWrites};
use medulla_core::lifecycle::derive::{CodeGraphView, SymbolFact, Span};
use medulla_core::lifecycle::store::LifecycleStore;
use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::source_changeset::{BlobIo, ChunkStoreBlobs};
use medulla_h_ingest::source_ingestion::spec_projection::parse_test_bindings;
use medulla_mutation_types::branch::RESERVED_BRANCH_NAME;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_blob::blob::store::ChunkStore;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{authorized_graph_read, ensure_graph_resident};
use medulla_source_types::lifecycle::{
    plan_changeset_node, ChangesetNodeWrite, LifecycleKind, StoredChangesetNode,
};
use medulla_source_types::source_changeset::{commit_node_id, Changeset, PathOp};
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;

/// The lifecycle kind a stored node stands for, by its `type` (or, for a run trace, `kind`).
fn kind_of(node: &Value) -> Option<LifecycleKind> {
    let name = node
        .get("type")
        .or_else(|| node.get("kind"))
        .and_then(Value::as_str)?;
    match name {
        "Requirement" => Some(LifecycleKind::Requirement),
        "CodeSymbolRef" | "SYMBOL" => Some(LifecycleKind::CodeSymbol),
        "TestCase" | "Test" => Some(LifecycleKind::TestCase),
        "Changeset" => Some(LifecycleKind::Changeset),
        "GateVerdict" => Some(LifecycleKind::GateVerdict),
        "RunTrace" => Some(LifecycleKind::RunTrace),
        "Release" => Some(LifecycleKind::Release),
        "Deployment" => Some(LifecycleKind::Deployment),
        "Defect" => Some(LifecycleKind::Defect),
        "Incident" => Some(LifecycleKind::Incident),
        _ => None,
    }
}

/// The caller's reading of one stored node: its decoded properties, absent when the node does
/// not exist or the caller may not see it.
fn visible(core: &GraphCore, authority: &GraphReadAuthority, id: &str) -> Option<Value> {
    let blob = core.get_node_properties(id)?;
    if !authority.can_see_node(&blob, core.is_schema_node(id)) {
        return None;
    }
    medulla_types::msgpack::decode_property_value(&blob).ok()
}

fn number(node: &Value, key: &str) -> Option<u32> {
    match node.get(key)? {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64().and_then(|n| u32::try_from(n).ok()),
        _ => None,
    }
}

/// The files of the changeset: the old and the new bytes of each changed path.
struct ChangedFile {
    path: String,
    old: Vec<u8>,
    new: Option<Vec<u8>>,
}

fn read_files(
    store: &dyn ChunkStore,
    owner: &str,
    changeset: &Changeset,
) -> Result<Vec<ChangedFile>, String> {
    let blobs = ChunkStoreBlobs(store, owner);
    changeset
        .changed_paths
        .iter()
        .map(|changed| {
            let old = match &changed.old_blob {
                Some(digest) => blobs.read(digest)?,
                None => Vec::new(),
            };
            let new = match (&changed.op, &changed.new_blob) {
                (PathOp::Delete, _) | (_, None) => None,
                (_, Some(digest)) => Some(blobs.read(digest)?),
            };
            Ok(ChangedFile {
                path: changed.path.clone(),
                old,
                new,
            })
        })
        .collect()
}

/// The code graph as `changeset` sees it: the symbols of its changed files with their byte
/// spans, the changed spans, and the test bindings its changed files declare whose
/// requirement and test the graph holds.
fn code_graph_view(
    core: &GraphCore,
    authority: &GraphReadAuthority,
    changeset: &Changeset,
    branch: &str,
    files: &[ChangedFile],
) -> CodeGraphView {
    let mut view = CodeGraphView {
        on_default_branch: branch == RESERVED_BRANCH_NAME,
        chain: vec![changeset.id.clone()],
        ..CodeGraphView::default()
    };
    let mut changed_spans: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for file in files {
        if let Some(new) = &file.new {
            if let Some(span) = changed_span(&file.old, new) {
                changed_spans.entry(file.path.clone()).or_default().push(span);
            }
        }
        for id in core
            .nodes_by_property("file_path", &file.path)
            .unwrap_or_default()
        {
            let Some(node) = visible(core, authority, &id) else {
                continue;
            };
            if node.get("type").and_then(Value::as_str) != Some("SYMBOL") {
                continue;
            }
            if let (Some(start), Some(end)) = (number(&node, "start_byte"), number(&node, "end_byte")) {
                view.symbols.entry(file.path.clone()).or_default().push(SymbolFact {
                    id,
                    path: file.path.clone(),
                    span: Span { start, end },
                });
            }
        }
    }
    view.changed_spans = changed_spans;
    let tree: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter_map(|file| Some((file.path.clone(), file.new.clone()?)))
        .collect();
    view.test_bindings = parse_test_bindings(&tree)
        .into_iter()
        .filter(|binding| {
            let requirement = format!("requirement:{}:{}", changeset.repo, binding.requirement);
            visible(core, authority, &requirement).is_some()
                && visible(core, authority, &binding.test).is_some()
        })
        .collect();
    view
}

fn stored_node(node: &Value) -> StoredChangesetNode {
    let text = |key: &str| {
        node.get(key).map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
    };
    StoredChangesetNode {
        changeset_id: text("changeset_id"),
        seq: text("seq"),
    }
}

/// The operations of one write set: the node, then each derived edge. The node is stamped with
/// its owner only when no node of that id exists, so the owner of an indexed commit stays.
fn operations(
    writes: &ChangesetWrites,
    verified: &VerifiedRequestContext,
    node_exists: bool,
) -> Vec<Value> {
    let node: &ChangesetNodeWrite = &writes.node;
    let mut properties: Map<String, Value> = node
        .properties()
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect();
    if !node_exists {
        properties.insert("tenant_id".into(), verified.tenant().into());
        properties.insert("_owner".into(), verified.agent_id().into());
    }
    let mut operations = vec![json!({"op": "upsert_node", "id": node.node_id, "properties": properties})];
    for edge in &writes.edges {
        let mut properties = Map::new();
        properties.insert("relationship".into(), edge.kind.name().into());
        properties.insert("rule".into(), edge.rule.clone().into());
        properties.insert("rule_version".into(), edge.rule_version.to_string().into());
        properties.insert("input_digests".into(), edge.input_digests.join(",").into());
        properties.insert("tenant_id".into(), verified.tenant().into());
        properties.insert("_owner".into(), verified.agent_id().into());
        operations.push(json!({
            "op": "upsert_edge",
            "source": edge.from,
            "target": edge.to,
            "properties": properties,
        }));
    }
    operations
}

/// Derive and write the lifecycle edges of the accepted `changeset`, carried at `seq` of its
/// chain on `branch`. `Ok(())` when the repository graph is not resident. A refusal
/// (`CHANGESET_NODE_CONFLICT`, `LIFECYCLE_ENDPOINT_UNKNOWN`) is returned as its coded text and
/// writes nothing; the write is idempotent, so a replay of the submission runs it again.
pub async fn accept_edges(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    req_id: u64,
    store: Arc<dyn ChunkStore>,
    owner: String,
    changeset: &Changeset,
    branch: &str,
    seq: u64,
) -> Result<(), String> {
    let repo = changeset.repo.clone();
    ensure_graph_resident(state, &repo).await;
    let resolved = {
        let current = state.read().await;
        authorized_graph_read(&current, &repo, verified, "GRAPH_NOT_FOUND")
    };
    let Ok((core, authority)) = resolved else {
        return Ok(());
    };
    let files = {
        let (changeset, owner) = (changeset.clone(), owner);
        tokio::task::spawn_blocking(move || read_files(store.as_ref(), &owner, &changeset))
            .await
            .map_err(|error| error.to_string())??
    };
    let node_id = commit_node_id(&changeset.id);
    let existing = visible(&core, &authority, &node_id);
    let stored = existing.as_ref().map(stored_node);
    let node = plan_changeset_node(changeset, seq, stored.as_ref(), true).map_err(|e| e.to_string())?;
    let view = code_graph_view(&core, &authority, changeset, branch, &files);
    let known = |id: &str| visible(&core, &authority, id).as_ref().and_then(kind_of);
    let writes = accept_changeset_edges(&mut LifecycleStore::default(), changeset, &view, node, &known)
        .map_err(|e| e.to_string())?;
    let operations = operations(&writes, verified, existing.is_some());
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "lifecycle batch encoding failed".to_string())?;
    let response = dispatch_graph_op(
        state,
        &repo,
        req_id,
        Some(verified.agent_id()),
        verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}
