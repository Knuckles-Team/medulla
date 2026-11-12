//! Served branches (MEDULLA-BRANCH-MERGE-R001).
//!
//! `CreateBranch` writes one row into the branch registry owner file
//! (`branch_registry.redb`, `OwnerLayout::BranchRegistry`) through that
//! owner's admitted native mutation. A branch copies no graph data. Every
//! refusal is decided before the registry write and writes nothing.

// MEDULLA-BRANCH-MERGE-R004 — entity-level three-way merge.
pub mod merge;

use std::sync::Arc;

use medulla_persistence::branch_registry_store::{
    shared_branch_registry, AppendError, AppendedCommit, BranchCommitInput, BranchCreateError,
    BranchRegistry, CommitPlan, PayloadReader,
};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::branch::{
    branch_target_graph, validate_branch_name, Branch, BranchMutationRefusal, BranchRef,
    BranchWriteReceipt, RESERVED_BRANCH_NAME,
};
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

/// The wire fields of `Method::CreateBranch`.
#[derive(Debug, Clone)]
pub struct CreateBranchRequest {
    pub tenant: String,
    pub graph: String,
    pub name: String,
    pub base_engine_version: Option<u64>,
}

fn refuse(req_id: u64, code: ServerErrorCode) -> Response {
    Response::err(req_id, format!("{}: create branch refused", code.as_str()))
}

/// Tenant and name checks, before any read of the graph or the registry.
fn validate(
    request: &CreateBranchRequest,
    verified: &VerifiedRequestContext,
) -> Result<(), ServerErrorCode> {
    if request.tenant != verified.tenant() {
        return Err(ServerErrorCode::BranchTenantDenied);
    }
    validate_branch_name(&request.name)
}

/// The base version: the graph head by default, never past it.
fn resolve_base(head: Option<u64>, requested: Option<u64>) -> Result<u64, ServerErrorCode> {
    let head = head.ok_or(ServerErrorCode::BranchBaseUnknown)?;
    let base = requested.unwrap_or(head);
    if base > head {
        return Err(ServerErrorCode::BranchBaseUnknown);
    }
    Ok(base)
}

/// The graph head version and the registry handle, read under one state lock.
async fn graph_head_and_registry(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
) -> Result<(Option<u64>, Arc<BranchRegistry>), String> {
    let s = state.read().await;
    let head = s.registry.get(graph).map(|entry| entry.core.version());
    Ok((head, shared_branch_registry(&s)?))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn new_branch(request: CreateBranchRequest, base: u64, created_by: &str) -> Branch {
    Branch {
        head_commit: format!("{}@{base}", request.graph),
        tenant: request.tenant,
        graph: request.graph,
        name: request.name,
        base_version: base,
        created_by: created_by.to_string(),
        created_at: now_ms(),
        deleted: false,
    }
}

/// Write the row off the async runtime.
async fn commit(
    registry: Arc<BranchRegistry>,
    branch: Branch,
) -> Result<Branch, BranchCreateError> {
    tokio::task::spawn_blocking(move || registry.create(&branch).map(|()| branch))
        .await
        .map_err(|error| BranchCreateError::Store(error.to_string()))
        .and_then(|outcome| outcome)
}

fn respond(req_id: u64, outcome: Result<Branch, BranchCreateError>) -> Response {
    let branch = match outcome {
        Ok(branch) => branch,
        Err(BranchCreateError::Refused(code)) => return refuse(req_id, code),
        Err(BranchCreateError::Store(detail)) => return Response::err(req_id, detail),
    };
    match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::CreateBranch>(
        branch,
    ) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// `Method::CreateBranch`: validate, resolve the base, write one registry row.
pub async fn handle_create_branch(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: CreateBranchRequest,
) -> Response {
    if let Err(code) = validate(&request, verified) {
        return refuse(req_id, code);
    }
    let (head, registry) = match graph_head_and_registry(state, &request.graph).await {
        Ok(found) => found,
        Err(error) => return Response::err(req_id, error),
    };
    let base = match resolve_base(head, request.base_engine_version) {
        Ok(base) => base,
        Err(code) => return refuse(req_id, code),
    };
    let branch = new_branch(request, base, verified.principal());
    respond(req_id, commit(registry, branch).await)
}

/// Where one branch-scoped mutation commits (MEDULLA-BRANCH-MERGE-R002).
///
/// Built only by [`resolve_target`], which refuses before any graph change.
#[derive(Debug, Clone)]
pub struct BranchTarget {
    pub reference: BranchRef,
    /// The graph the branch's writes live in; never the base graph.
    pub target_graph: String,
    registry: Arc<BranchRegistry>,
}

/// Why a branch-scoped write could not resolve its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchTargetError {
    /// A typed refusal decided before any graph change.
    Refused(BranchMutationRefusal),
    /// The registry itself failed.
    Store(String),
}

/// Map `(tenant, graph, branch)` to the commit target before the mutation
/// compiles (MEDULLA-BRANCH-MERGE-R002). The request tenant must be the
/// verified tenant; the trunk `main` is not a branch row and is refused as
/// `BRANCH_UNKNOWN` (a trunk write names no branch); an unknown, deleted or
/// foreign-tenant branch refuses with its typed code.
pub async fn resolve_target(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    reference: BranchRef,
) -> Result<BranchTarget, BranchTargetError> {
    if reference.tenant != verified.tenant() {
        return Err(BranchTargetError::Refused(
            BranchMutationRefusal::BranchTenantDenied,
        ));
    }
    if reference.name == RESERVED_BRANCH_NAME {
        return Err(BranchTargetError::Refused(
            BranchMutationRefusal::BranchUnknown,
        ));
    }
    let registry = {
        let s = state.read().await;
        shared_branch_registry(&s).map_err(BranchTargetError::Store)?
    };
    let lookup = {
        let registry = Arc::clone(&registry);
        let reference = reference.clone();
        let tenant = verified.tenant().to_string();
        tokio::task::spawn_blocking(move || registry.resolve_live(&reference, &tenant))
            .await
            .map_err(|error| BranchTargetError::Store(error.to_string()))?
            .map_err(BranchTargetError::Store)?
    };
    lookup.map_err(BranchTargetError::Refused)?;
    Ok(BranchTarget {
        target_graph: branch_target_graph(&reference),
        reference,
        registry,
    })
}

impl BranchTarget {
    /// The commit a prior attempt with `batch_id` already appended.
    pub async fn replayed(
        &self,
        batch_id: &str,
    ) -> Result<Option<medulla_types::branch::BranchCommit>, String> {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        let batch_id = batch_id.to_string();
        tokio::task::spawn_blocking(move || registry.commit_for_batch(&reference, &batch_id))
            .await
            .map_err(|error| error.to_string())?
    }

    /// Append the commit and advance the head in one registry transaction.
    pub async fn append(
        &self,
        tenant: &str,
        input: BranchCommitInput,
    ) -> Result<AppendedCommit, BranchCreateError> {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        let tenant = tenant.to_string();
        tokio::task::spawn_blocking(move || registry.append_commit(&reference, &tenant, &input))
            .await
            .map_err(|error| BranchCreateError::Store(error.to_string()))?
    }

    /// The payload a commit carries (a source changeset, MEDULLA-SCM-R002).
    pub async fn payload(&self, commit_id: &str) -> Result<Option<Vec<u8>>, String> {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        let commit_id = commit_id.to_string();
        tokio::task::spawn_blocking(move || registry.commit_payload(&reference, &commit_id))
            .await
            .map_err(|error| error.to_string())?
    }

    /// Append a commit built inside the registry write transaction from the
    /// head it extends, with its payload (MEDULLA-SCM-R002).
    pub async fn append_with<F>(
        &self,
        tenant: &str,
        batch_id: &str,
        build: F,
    ) -> Result<AppendedCommit, AppendError>
    where
        F: FnOnce(&str, PayloadReader<'_>) -> Result<CommitPlan, String> + Send + 'static,
    {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        let tenant = tenant.to_string();
        let batch_id = batch_id.to_string();
        tokio::task::spawn_blocking(move || {
            registry.append_with(&reference, &tenant, &batch_id, build)
        })
        .await
        .map_err(|error| AppendError::Branch(BranchCreateError::Store(error.to_string())))?
    }

    /// The served receipt of an appended (or replayed) commit.
    pub fn receipt(&self, appended: AppendedCommit) -> BranchWriteReceipt {
        BranchWriteReceipt {
            branch_head: appended.commit.id.clone(),
            commit: appended.commit,
            target_graph: self.target_graph.clone(),
            replayed: appended.replayed,
        }
    }
}

/// The refusal response of a failed target resolution.
pub fn target_error_response(req_id: u64, error: BranchTargetError) -> Response {
    match error {
        BranchTargetError::Refused(refusal) => Response::err(
            req_id,
            format!("{}: branch write refused", refusal.code().as_str()),
        ),
        BranchTargetError::Store(detail) => Response::err(req_id, detail),
    }
}

/// The refusal response of a failed commit append.
pub fn append_error_response(req_id: u64, error: BranchCreateError) -> Response {
    match error {
        BranchCreateError::Refused(code) => {
            Response::err(req_id, format!("{}: branch write refused", code.as_str()))
        }
        BranchCreateError::Store(detail) => Response::err(req_id, detail),
    }
}

/// A closed refusal for a `mutation` that is not one graph mutation.
pub fn invalid_write(req_id: u64, detail: &str) -> Response {
    Response::err(
        req_id,
        format!("{}: {detail}", ServerErrorCode::BranchWriteInvalid.as_str()),
    )
}

/// The payload of a served branch receipt.
pub fn receipt_response(req_id: u64, receipt: BranchWriteReceipt) -> Response {
    match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BranchWrite>(
        receipt,
    ) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The digest recorded as the commit manifest: SHA-256 of the mutation's
/// canonical named MessagePack.
pub fn mutation_manifest_digest(
    mutation: &medulla_protocol_types::protocol::Method,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = rmp_serde::to_vec_named(mutation).map_err(|error| error.to_string())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-BRANCH-MERGE-R001
    #[test]
    fn base_defaults_to_head_and_never_passes_it() {
        // (head, requested base, expected base)
        type Case = (Option<u64>, Option<u64>, Result<u64, ServerErrorCode>);
        let cases: &[Case] = &[
            (Some(7), None, Ok(7)),
            (Some(7), Some(3), Ok(3)),
            (Some(7), Some(7), Ok(7)),
            (Some(7), Some(8), Err(ServerErrorCode::BranchBaseUnknown)),
            (None, None, Err(ServerErrorCode::BranchBaseUnknown)),
        ];
        for (head, requested, expected) in cases {
            assert_eq!(&resolve_base(*head, *requested), expected);
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn branch_batch_ids_are_scoped_by_tenant_graph_and_branch() {
        use medulla_server_api::batch_compile::branch_batch_id;
        let base = branch_batch_id("t", "g", "a", Some("p"), "k");
        assert_eq!(base, branch_batch_id("t", "g", "a", Some("p"), "k"));
        assert_ne!(base, branch_batch_id("t2", "g", "a", Some("p"), "k"));
        assert_ne!(base, branch_batch_id("t", "g", "b", Some("p"), "k"));
        assert_ne!(base, branch_batch_id("t", "g", "a", Some("p"), "k2"));
        assert_ne!(
            branch_batch_id("t", "g", "ab", Some("p"), "k"),
            branch_batch_id("t", "ga", "b", Some("p"), "k")
        );
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn manifest_digest_binds_the_mutation() {
        use medulla_protocol_types::protocol::Method;
        let a = Method::HasNode {
            node_id: "a".into(),
        };
        let b = Method::HasNode {
            node_id: "b".into(),
        };
        assert_eq!(
            mutation_manifest_digest(&a).unwrap(),
            mutation_manifest_digest(&a).unwrap()
        );
        assert_ne!(
            mutation_manifest_digest(&a).unwrap(),
            mutation_manifest_digest(&b).unwrap()
        );
    }
}
