//! The served side of the lifecycle derivation of an accepted changeset
//! (MEDULLA-SDLC-R001.3): plan the changeset node, derive the edges through
//! `accept_changeset_edges`, and apply both as ONE `BatchUpdate` through the ordinary graph
//! gateway, the node ahead of its edges. The write is ACL-checked, sealed and durable before
//! the receipt returns. A repository graph that is not resident has no lifecycle to keep.
//!
//! Both routes run it: `SubmitChangeset` for the changeset it accepted, and the
//! `IndexRepository` route for each changeset of the branch chain it follows. The code graph
//! view reads `test_calls`, `releases` and `culprits` from the graph, so both routes see the
//! same view. A trailer edge whose record is not ingested yet waits in a bucket node
//! (`unresolved:<record node id>`) and becomes the edge when the record is ingested
//! ([`resolve_waiting`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::dispatch::graph_pipeline::dispatch_graph_op;
use crate::dispatch::shared::{authorized_graph_read, ensure_graph_resident};
use medulla_core::lifecycle::accept::{
    accept_changeset_edges, changed_span, unresolved_node_id, ChangesetWrites, UnresolvedReference,
    AWAITS, UNRESOLVED_TYPE,
};
use medulla_core::lifecycle::derive::{
    CodeGraphView, Culprit, DerivedEdge, ReleaseHead, Span, SymbolFact, TestCall,
};
use medulla_core::lifecycle::store::LifecycleStore;
use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::source_changeset::{BlobIo, ChunkStoreBlobs};
use medulla_h_ingest::source_ingestion::spec_projection::{parse_test_bindings, test_file_node_id};
use medulla_mutation_types::branch::{BranchRef, RESERVED_BRANCH_NAME};
use medulla_persistence::branch_registry_store::BranchRegistry;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_blob::blob::store::ChunkStore;
use medulla_source_types::lifecycle::{
    plan_changeset_node, ChangesetNodeWrite, LifecycleKind, StoredChangesetNode,
};
use medulla_source_types::source_changeset::{commit_node_id, decode_changeset, Changeset, PathOp};
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;

/// The lifecycle kind a stored node stands for, by its `type` (or, for a run trace, `kind`).
fn kind_of(node: &Value) -> Option<LifecycleKind> {
    let name = node
        .get("type")
        .or_else(|| node.get("kind"))
        .and_then(Value::as_str)?;
    LifecycleKind::from_node_type(name)
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

fn number64(node: &Value, key: &str) -> Option<u64> {
    match node.get(key)? {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// The branch the changeset chain is read from: its registry and reference.
#[derive(Clone)]
pub struct BranchChain {
    registry: Arc<BranchRegistry>,
    reference: BranchRef,
}

impl BranchChain {
    pub fn new(registry: Arc<BranchRegistry>, reference: BranchRef) -> Self {
        Self {
            registry,
            reference,
        }
    }

    /// The ids of the changesets on the branch chain, oldest first (blocking).
    pub fn changeset_ids(&self) -> Result<Vec<String>, String> {
        let mut ids = Vec::new();
        for commit in self.registry.chain(&self.reference)? {
            let Some(bytes) = self.registry.commit_payload(&self.reference, &commit.id)? else {
                continue;
            };
            if let Ok(changeset) = decode_changeset(&bytes) {
                ids.push(changeset.id);
            }
        }
        ids.reverse();
        Ok(ids)
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
                changed_spans
                    .entry(file.path.clone())
                    .or_default()
                    .push(span);
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
            if let (Some(start), Some(end)) =
                (number(&node, "start_byte"), number(&node, "end_byte"))
            {
                view.symbols
                    .entry(file.path.clone())
                    .or_default()
                    .push(SymbolFact {
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
    // A bound test is the `testfile:<repo>:<path>` node of its file.
    view.test_bindings = parse_test_bindings(&tree)
        .into_iter()
        .filter_map(|mut binding| {
            let requirement = format!("requirement:{}:{}", changeset.repo, binding.requirement);
            let test = test_file_node_id(&changeset.repo, &binding.test);
            if visible(core, authority, &requirement).is_none()
                || visible(core, authority, &test).is_none()
            {
                return None;
            }
            binding.test = test;
            Some(binding)
        })
        .collect();
    view.test_calls = test_calls(core, authority, changeset, &view);
    view.releases = releases(core, authority, &changeset.repo);
    view.culprits = culprits(core, authority, changeset);
    view
}

/// The calls from the bound test files to code symbols, read from the stored `calls` edges of
/// the symbols each test file defines.
fn test_calls(
    core: &GraphCore,
    authority: &GraphReadAuthority,
    changeset: &Changeset,
    view: &CodeGraphView,
) -> Vec<TestCall> {
    let mut calls = std::collections::BTreeSet::new();
    for binding in &view.test_bindings {
        let prefix = test_file_node_id(&changeset.repo, "");
        let Some(path) = binding.test.strip_prefix(&prefix) else {
            continue;
        };
        for caller in view.symbols.get(path).into_iter().flatten() {
            for callee in core.get_successors(&caller.id).unwrap_or_default() {
                let called = core
                    .typed_edges_between(&caller.id, &callee)
                    .iter()
                    .any(|(relation, _)| relation == "calls");
                let symbol = visible(core, authority, &callee)
                    .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some("SYMBOL"));
                if called && symbol {
                    calls.insert((binding.test.clone(), callee));
                }
            }
        }
    }
    calls
        .into_iter()
        .map(|(test, symbol)| TestCall { test, symbol })
        .collect()
}

/// The release heads of the repository, each with the head of the release before it by
/// publication time.
fn releases(core: &GraphCore, authority: &GraphReadAuthority, repo: &str) -> Vec<ReleaseHead> {
    let mut found: Vec<(u64, String, String)> = core
        .nodes_by_property("repo", repo)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let node = visible(core, authority, &id)?;
            if node.get("type").and_then(Value::as_str) != Some("Release") {
                return None;
            }
            let head = node.get("changeset_head")?.as_str()?.to_owned();
            Some((number64(&node, "published_at").unwrap_or(0), id, head))
        })
        .collect();
    found.sort();
    let mut previous: Option<String> = None;
    found
        .into_iter()
        .map(|(_, release, head)| {
            let entry = ReleaseHead {
                release,
                head: head.clone(),
                previous_head: previous.take(),
            };
            previous = Some(head);
            entry
        })
        .collect()
}

/// The defects whose bisect culprit record names this changeset: a `Defect` node of the
/// repository holding `culprit_changeset`.
fn culprits(
    core: &GraphCore,
    authority: &GraphReadAuthority,
    changeset: &Changeset,
) -> Vec<Culprit> {
    let prefix = format!("defect:{}/", changeset.repo);
    core.nodes_by_property("culprit_changeset", &changeset.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|id| {
            visible(core, authority, id)
                .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some("Defect"))
        })
        .filter_map(|id| {
            Some(Culprit {
                defect: id.strip_prefix(&prefix)?.to_owned(),
                changeset: changeset.id.clone(),
            })
        })
        .collect()
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
    let mut operations =
        vec![json!({"op": "upsert_node", "id": node.node_id, "properties": properties})];
    operations.extend(
        writes
            .edges
            .iter()
            .map(|edge| edge_operation(edge, verified)),
    );
    // A reference whose record is not ingested yet waits in its bucket.
    for reference in &writes.unresolved {
        let bucket = reference.bucket_id();
        operations.push(json!({
            "op": "upsert_node",
            "id": bucket,
            "properties": {
                "type": UNRESOLVED_TYPE,
                "target": reference.missing,
                "tenant_id": verified.tenant(),
                "_owner": verified.agent_id(),
            },
        }));
        let mut properties: Map<String, Value> = reference
            .waiting_properties()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Value::String(value)))
            .collect();
        properties.insert("tenant_id".into(), verified.tenant().into());
        properties.insert("_owner".into(), verified.agent_id().into());
        operations.push(json!({
            "op": "upsert_edge",
            "source": node.node_id,
            "target": bucket,
            "properties": properties,
        }));
    }
    operations
}

/// The `upsert_edge` operation of one derived edge between graph node ids.
fn edge_operation(edge: &DerivedEdge, verified: &VerifiedRequestContext) -> Value {
    let mut properties = Map::new();
    properties.insert("relationship".into(), edge.kind.name().into());
    properties.insert("rule".into(), edge.rule.clone().into());
    properties.insert("rule_version".into(), edge.rule_version.to_string().into());
    properties.insert("input_digests".into(), edge.input_digests.join(",").into());
    properties.insert("tenant_id".into(), verified.tenant().into());
    properties.insert("_owner".into(), verified.agent_id().into());
    json!({
        "op": "upsert_edge",
        "source": edge.from,
        "target": edge.to,
        "properties": properties,
    })
}

async fn apply(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    req_id: u64,
    repo: &str,
    operations: Vec<Value>,
) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "lifecycle batch encoding failed".to_string())?;
    let response = Box::pin(dispatch_graph_op(
        state,
        repo,
        req_id,
        Some(verified.agent_id()),
        verified,
        Method::BatchUpdate { operations_msgpack },
    ))
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// Resolve the references that wait on the records `records` (graph node ids) now that they
/// are ingested: each waiting edge whose two endpoints exist is written as its derived edge,
/// then its bucket is removed. A reference whose other endpoint is still absent keeps
/// waiting. `Ok(())` when the graph is not resident or nothing waits.
pub async fn resolve_waiting(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    req_id: u64,
    repo: &str,
    records: &[String],
) -> Result<(), String> {
    ensure_graph_resident(state, repo).await;
    let resolved = {
        let current = state.read().await;
        authorized_graph_read(&current, repo, verified, "GRAPH_NOT_FOUND")
    };
    let Ok((core, authority)) = resolved else {
        return Ok(());
    };
    let mut operations = Vec::new();
    for record in records {
        let bucket = unresolved_node_id(record);
        if visible(&core, &authority, record).is_none() {
            continue;
        }
        let mut drained = true;
        for waiter in core.get_predecessors(&bucket).unwrap_or_default() {
            for blob in core.get_edge_properties(&waiter, &bucket) {
                let Ok(properties) = medulla_types::msgpack::decode_property_value(&blob) else {
                    continue;
                };
                if properties.get("relationship").and_then(Value::as_str) != Some(AWAITS) {
                    continue;
                }
                let get = |key: &str| {
                    properties
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                let Some(edge) = UnresolvedReference::resolve(&get) else {
                    continue;
                };
                let both = [&edge.from, &edge.to]
                    .iter()
                    .all(|id| visible(&core, &authority, id).is_some());
                if both {
                    operations.push(edge_operation(&edge, verified));
                } else {
                    drained = false;
                }
            }
        }
        if drained {
            operations.push(json!({"op": "remove_node", "id": bucket}));
        }
    }
    if operations.is_empty() {
        return Ok(());
    }
    apply(state, verified, req_id, repo, operations).await
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
    chain: BranchChain,
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
    let node =
        plan_changeset_node(changeset, seq, stored.as_ref(), true).map_err(|e| e.to_string())?;
    let mut view = code_graph_view(&core, &authority, changeset, branch, &files);
    // A release range is read from the whole chain, so it is walked only when the graph holds
    // a release.
    if !view.releases.is_empty() {
        view.chain = tokio::task::spawn_blocking(move || chain.changeset_ids())
            .await
            .map_err(|error| error.to_string())??;
    }
    let known = |id: &str| visible(&core, &authority, id).as_ref().and_then(kind_of);
    let writes = accept_changeset_edges(
        &mut LifecycleStore::default(),
        changeset,
        &view,
        node,
        &known,
    )
    .map_err(|e| e.to_string())?;
    let operations = operations(&writes, verified, existing.is_some());
    apply(state, verified, req_id, &repo, operations).await
}
