//! Branch-scoped mutations on one commit chain (MEDULLA-BRANCH-MERGE-R002).
//!
//! A request names a branch in its signed graph reference,
//! `<graph>@branch/<name>` (D111 v2); the router parses it once at admission.
//! `resolve_target` maps `(tenant, graph, branch)` to the registry row before
//! anything runs, refusing `BRANCH_UNKNOWN`, `BRANCH_DELETED` or
//! `BRANCH_TENANT_DENIED` with no change anywhere. The write then runs on a
//! copy-on-write branch view (a fork of the graph image with the branch's
//! committed write sets applied in order); the live graph is never touched, so
//! `main` and sibling branches do not see it. The write set is appended to the
//! branch commit log as one `BranchCommit` whose parent is the prior head, and
//! the registry head moves to it, in one admitted write before the ack. A
//! retry under the same `(tenant, graph, branch, idempotency_key)` returns the
//! original receipt and writes nothing.

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_isolation::isolation::AccessLevel;
use medulla_persistence::branch_registry_store::{shared_branch_registry, BranchRegistry};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::branch::{
    branch_idempotency_key, Branch, BranchCommit, BranchRef, BranchWriteReceipt, StoredBranchCommit,
};
use medulla_types::contract::ServerErrorCode;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// The served writes a branch view applies: the methods whose RAM apply is a
/// pure function of the graph image (`apply_coalescable_write`). Any other
/// method on a branch reference refuses `BRANCH_METHOD_UNSUPPORTED`.
const BRANCH_WRITE_METHODS: &[&str] = &[
    "AddEdge",
    "AddNode",
    "CompareAndSetNodeFields",
    "RemoveEdge",
    "RemoveNode",
];

/// One branch-scoped request, after admission parsed its graph reference.
pub struct BranchWrite<'a> {
    pub req_id: u64,
    pub graph: &'a str,
    pub branch: &'a str,
    pub verified: &'a VerifiedRequestContext,
    pub method: Method,
}

/// What a branch write needs from the server state, read under one lock.
struct BranchScope {
    registry: Arc<BranchRegistry>,
    base: Arc<GraphCore>,
    reference: BranchRef,
    row: Branch,
}

fn refuse(req_id: u64, code: ServerErrorCode) -> Response {
    Response::err(req_id, format!("{}: branch write refused", code.as_str()))
}

fn admit_method(method: &Method) -> Result<(), ServerErrorCode> {
    BRANCH_WRITE_METHODS
        .contains(&<&str>::from(method))
        .then_some(())
        .ok_or(ServerErrorCode::BranchMethodUnsupported)
}

/// Map `(tenant, graph, branch)` to its live registry row, or the refusal.
pub fn resolve_target(
    registry: &BranchRegistry,
    reference: &BranchRef,
) -> Result<Branch, ServerErrorCode> {
    let row = registry
        .get(reference)
        .map_err(|_| ServerErrorCode::BranchUnknown)?
        .ok_or(ServerErrorCode::BranchUnknown)?;
    if row.deleted {
        return Err(ServerErrorCode::BranchDeleted);
    }
    Ok(row)
}

/// The caller may write the bare graph, or the branch write is denied.
fn admit_graph_write(
    s: &ServerState,
    request: &BranchWrite<'_>,
) -> Result<Arc<GraphCore>, ServerErrorCode> {
    let record = s
        .registry
        .catalog_record(request.graph)
        .ok_or(ServerErrorCode::BranchUnknown)?;
    medulla_server_auth::access::check_graph_access(
        &s.isolation,
        Some(request.verified.agent_id()),
        request.graph,
        record.graph_type,
        record.owner.as_deref(),
        AccessLevel::Write,
    )
    .map_err(|_| ServerErrorCode::BranchTenantDenied)?;
    s.registry
        .get(request.graph)
        .map(|entry| entry.core.clone())
        .ok_or(ServerErrorCode::BranchUnknown)
}

async fn scope(
    state: &Arc<RwLock<ServerState>>,
    request: &BranchWrite<'_>,
) -> Result<BranchScope, Response> {
    let refused = |code| refuse(request.req_id, code);
    admit_method(&request.method).map_err(refused)?;
    let s = state.read().await;
    let base = admit_graph_write(&s, request).map_err(refused)?;
    let registry = shared_branch_registry(&s).map_err(|e| Response::err(request.req_id, e))?;
    drop(s);
    let reference = BranchRef {
        tenant: request.verified.tenant().to_string(),
        graph: request.graph.to_string(),
        name: request.branch.to_string(),
    };
    let row = resolve_target(&registry, &reference).map_err(refused)?;
    Ok(BranchScope {
        registry,
        base,
        reference,
        row,
    })
}

fn decode_write_set(bytes: &[u8]) -> Result<Method, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

/// The copy-on-write branch view: a fork of `base` with every committed write
/// set of the branch applied in commit order.
pub fn branch_view(base: &GraphCore, commits: &[StoredBranchCommit]) -> Result<GraphCore, String> {
    let view = base.fork();
    for stored in commits {
        let method = decode_write_set(&stored.write_set)?;
        medulla_server_mutation::mutation::apply_coalescable_write(&view, &method)?;
    }
    Ok(view)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn hex_sha256(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

/// The next commit on the chain: its parent is the prior head.
fn next_commit(scope: &BranchScope, sequence: u64, write_set: &[u8], actor: &str) -> BranchCommit {
    let manifest_digest = hex_sha256(&[write_set]);
    let parent = scope.row.head_commit.clone();
    let id = hex_sha256(&[
        scope.reference.registry_key().as_bytes(),
        parent.as_bytes(),
        &sequence.to_be_bytes(),
        manifest_digest.as_bytes(),
    ]);
    BranchCommit {
        id,
        branch: scope.reference.name.clone(),
        parents: vec![parent],
        engine_version: scope.row.base_version.saturating_add(sequence + 1),
        manifest_digest,
        actor: actor.to_string(),
        created_at: now_ms(),
    }
}

fn receipt_response(
    req_id: u64,
    stored: &StoredBranchCommit,
    replayed: bool,
) -> Result<Response, String> {
    let result: ResultPayload =
        rmp_serde::from_slice(&stored.result).map_err(|error| error.to_string())?;
    let receipt = BranchWriteReceipt {
        branch_head: stored.commit.id.clone(),
        commit: stored.commit.clone(),
        replayed,
    };
    let body = serde_json::json!({ "result": result, "receipt": receipt });
    Ok(Response::ok(req_id, ResultPayload::Json(body)))
}

/// Apply the write on the branch view and append its commit.
fn commit_on_branch(
    scope: &BranchScope,
    method: &Method,
    idempotency_key: &str,
    actor: &str,
) -> Result<(StoredBranchCommit, bool), String> {
    let commits = scope.registry.commits(&scope.reference)?;
    if let Some(prior) = commits
        .iter()
        .find(|stored| stored.idempotency_key == idempotency_key)
    {
        return Ok((prior.clone(), true));
    }
    let view = branch_view(&scope.base, &commits)?;
    let result = medulla_server_mutation::mutation::apply_coalescable_write(&view, method)?;
    let write_set = rmp_serde::to_vec_named(method).map_err(|error| error.to_string())?;
    let sequence = commits.len() as u64;
    let stored = StoredBranchCommit {
        commit: next_commit(scope, sequence, &write_set, actor),
        idempotency_key: idempotency_key.to_string(),
        result: rmp_serde::to_vec_named(&result).map_err(|error| error.to_string())?,
        write_set,
    };
    scope
        .registry
        .append_commit(&scope.reference, sequence, &stored)?;
    Ok((stored, false))
}

/// A served mutation on `<graph>@branch/<name>`: admit, apply on the branch
/// view, append one commit, answer with the method result and the receipt.
pub async fn handle_branch_write(
    state: &Arc<RwLock<ServerState>>,
    request: BranchWrite<'_>,
) -> Response {
    let scope = match scope(state, &request).await {
        Ok(scope) => scope,
        Err(refusal) => return refusal,
    };
    let key = branch_idempotency_key(&scope.reference, request.verified.idempotency_key());
    let lane = format!("{}#branch", scope.reference.registry_key());
    let _lane = medulla_server_api::routed_write_coalescer::lock_graph(&lane).await;
    let actor = request.verified.principal().to_string();
    let method = request.method;
    let outcome =
        tokio::task::spawn_blocking(move || commit_on_branch(&scope, &method, &key, &actor))
            .await
            .map_err(|error| error.to_string())
            .and_then(|outcome| outcome)
            .and_then(|(stored, replayed)| receipt_response(request.req_id, &stored, replayed));
    outcome.unwrap_or_else(|error| Response::err(request.req_id, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_node(id: &str) -> Method {
        Method::AddNode {
            node_id: id.to_string(),
            properties_msgpack: Vec::new(),
        }
    }

    fn stored(method: &Method) -> StoredBranchCommit {
        StoredBranchCommit {
            commit: BranchCommit {
                id: "c".into(),
                branch: "b".into(),
                parents: vec![],
                engine_version: 1,
                manifest_digest: String::new(),
                actor: "a".into(),
                created_at: 0,
            },
            idempotency_key: "k".into(),
            write_set: rmp_serde::to_vec_named(method).unwrap(),
            result: Vec::new(),
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn branch_view_applies_commits_and_leaves_the_base_unchanged() {
        let base = GraphCore::new();
        let commits = vec![stored(&add_node("n1")), stored(&add_node("n2"))];
        let view = branch_view(&base, &commits).unwrap();
        assert_eq!(view.node_count(), 2);
        assert_eq!(base.node_count(), 0);
    }

    #[test]
    fn only_pure_graph_writes_run_on_a_branch() {
        assert_eq!(admit_method(&add_node("n")), Ok(()));
        assert_eq!(
            admit_method(&Method::ClearGraph),
            Err(ServerErrorCode::BranchMethodUnsupported)
        );
    }
}
