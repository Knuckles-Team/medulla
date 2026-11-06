//! Served branches (MEDULLA-BRANCH-MERGE-R001).
//!
//! `CreateBranch` writes one row into the branch registry owner file
//! (`branch_registry.redb`, `OwnerLayout::BranchRegistry`) through that
//! owner's admitted native mutation. A branch copies no graph data. Every
//! refusal is decided before the registry write and writes nothing.

use std::sync::Arc;

use medulla_persistence::branch_registry_store::{BranchCreateError, BranchRegistry};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::{ErasedHandle, ServerState};
use medulla_types::branch::{validate_branch_name, Branch};
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

/// Open (once) the shared branch registry under the persist dir.
fn open_registry(s: &ServerState, persist_dir: &str) -> Result<Arc<BranchRegistry>, String> {
    let mut guard = s.branch_registry.lock();
    if let Some(registry) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<BranchRegistry>().ok())
    {
        return Ok(registry);
    }
    let registry = Arc::new(BranchRegistry::open(persist_dir)?);
    *guard = Some(Arc::clone(&registry) as ErasedHandle);
    Ok(registry)
}

/// The graph head version and the registry handle, read under one state lock.
async fn graph_head_and_registry(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
) -> Result<(Option<u64>, Arc<BranchRegistry>), String> {
    let s = state.read().await;
    let head = s.registry.get(graph).map(|entry| entry.core.version());
    let persist_dir = s
        .persist_dir
        .as_deref()
        .ok_or_else(|| "branch registry requires a configured persist directory".to_string())?;
    Ok((head, open_registry(&s, persist_dir)?))
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

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-BRANCH-MERGE-R001
    #[test]
    fn base_defaults_to_head_and_never_passes_it() {
        let cases: &[(Option<u64>, Option<u64>, Result<u64, ServerErrorCode>)] = &[
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
}
