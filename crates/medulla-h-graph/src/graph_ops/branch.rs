//! Served branches (MEDULLA-BRANCH-MERGE-R001).
//!
//! `CreateBranch` writes one row into the branch registry owner file
//! (`branch_registry.redb`, `OwnerLayout::BranchRegistry`) through that
//! owner's admitted native mutation. A branch copies no graph data. Every
//! refusal is decided before the registry write and writes nothing.

// MEDULLA-BRANCH-MERGE-R004 — entity-level three-way merge.
pub mod merge;
// MEDULLA-BRANCH-MERGE-R005 — epistemic conflict resolution.
pub mod conflict;
// MEDULLA-BRANCH-MERGE-R010 — line-granularity file merge for source changesets.
pub mod line_merge;
// MEDULLA-BRANCH-MERGE-R006 — branch diff and time-travel reads.
pub mod time_travel;
// MEDULLA-BRANCH-MERGE-R007 — atomic merge publication.
pub mod publish;

use std::sync::Arc;

use medulla_mutation_types::branch::{
    branch_target_graph, classify, merge_base, validate_branch_name, Branch,
    BranchMergeReceipt, BranchMutationRefusal, BranchRef, BranchWriteReceipt, Head, MergeOutcome,
    RESERVED_BRANCH_NAME,
};
use medulla_persistence::branch_registry_store::{
    shared_branch_registry, AppendError, AppendedCommit, BranchCommitInput, BranchCreateError,
    BranchRegistry, CommitPlan, HeadMoveError, HistoryReader, MergeView, MergeWrite, PayloadReader,
};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
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

/// The wire fields of `Method::BranchMerge` (MEDULLA-BRANCH-MERGE-R003).
#[derive(Debug, Clone)]
pub struct BranchMergeRequest {
    pub tenant: String,
    pub graph: String,
    pub source: String,
    pub target: String,
    /// The base commit the caller planned against, if it pinned one.
    pub merge_base: Option<String>,
    /// The policy the conflicts resolve under; `None` is the engine default.
    pub policy: Option<medulla_decision_types::decision::DecisionPolicyRef>,
}

/// The classified heads of one merge, before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeClassification {
    pub outcome: MergeOutcome,
    pub merge_base: String,
    pub source_head: String,
    pub target_head: String,
}

/// Why a merge classification refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeClassifyError {
    Refused(ServerErrorCode),
    Store(String),
}

fn merge_error_response(req_id: u64, error: MergeClassifyError) -> Response {
    match error {
        MergeClassifyError::Refused(code) => {
            Response::err(req_id, format!("{}: branch merge refused", code.as_str()))
        }
        MergeClassifyError::Store(detail) => Response::err(req_id, detail),
    }
}

fn publish_error_response(req_id: u64, error: publish::PublishError) -> Response {
    match error {
        publish::PublishError::Refused(code) => {
            Response::err(req_id, format!("{}: branch merge refused", code.as_str()))
        }
        publish::PublishError::AdoptionInvalid(message) => Response::err(req_id, message),
        publish::PublishError::Failed(detail) => Response::err(req_id, detail),
    }
}

/// The receipt of an `up_to_date` merge, which writes nothing.
fn plain_receipt(classified: MergeClassification) -> BranchMergeReceipt {
    BranchMergeReceipt {
        outcome: classified.outcome,
        merge_base: classified.merge_base,
        source_head: classified.source_head,
        target_head: classified.target_head,
        merged_parent: None,
        commit: None,
        conflicts: Vec::new(),
        replayed: false,
    }
}

fn merge_dag(view: &MergeView) -> Vec<(String, u64, Vec<String>)> {
    view.commits
        .iter()
        .map(|commit| (commit.id.clone(), commit.generation, commit.parents.clone()))
        .collect()
}

/// The pure classification of two branch views (MEDULLA-BRANCH-MERGE-R003):
/// the merge base on the union of both commit DAGs, then the closed outcome.
/// No common ancestor refuses `BRANCH_NO_COMMON_ANCESTOR`; a caller-pinned
/// base that differs from the computed one refuses `MERGE_TARGET_STALE`.
pub fn classify_merge(
    source: &MergeView,
    target: &MergeView,
    pinned_base: Option<&str>,
) -> Result<MergeClassification, MergeClassifyError> {
    let mut dag = merge_dag(source);
    for entry in merge_dag(target) {
        if !dag.iter().any(|(id, _, _)| *id == entry.0) {
            dag.push(entry);
        }
    }
    let source_head = Head {
        id: source.head.clone(),
        generation: source.generation,
    };
    let target_head = Head {
        id: target.head.clone(),
        generation: target.generation,
    };
    let base = merge_base(&dag, &source_head, &target_head);
    let outcome = classify(&source_head, &target_head, base.clone()).ok_or(
        MergeClassifyError::Refused(ServerErrorCode::BranchNoCommonAncestor),
    )?;
    // Equal heads need no base: the head is its own ancestor.
    let base_id = base.map_or_else(|| source.head.clone(), |(id, _)| id);
    if pinned_base.is_some_and(|pinned| pinned != base_id) {
        return Err(MergeClassifyError::Refused(
            ServerErrorCode::MergeTargetStale,
        ));
    }
    Ok(MergeClassification {
        outcome,
        merge_base: base_id,
        source_head: source.head.clone(),
        target_head: target.head.clone(),
    })
}

/// Read both views and classify them on a blocking thread.
fn classify_branches(
    registry: &BranchRegistry,
    source: &BranchRef,
    target: &BranchRef,
    tenant: &str,
    pinned_base: Option<&str>,
) -> Result<MergeClassification, MergeClassifyError> {
    let mut views = Vec::with_capacity(2);
    for reference in [source, target] {
        let view = registry
            .merge_view(reference, tenant)
            .map_err(MergeClassifyError::Store)?
            .map_err(|refusal| MergeClassifyError::Refused(refusal.code()))?;
        views.push(view);
    }
    let missing = || MergeClassifyError::Store("merge view missing".into());
    let target_view = views.pop().ok_or_else(missing)?;
    let source_view = views.pop().ok_or_else(missing)?;
    classify_merge(&source_view, &target_view, pinned_base)
}

/// `Method::BranchMerge` (MEDULLA-BRANCH-MERGE-R003): resolve both branches,
/// find the merge base by highest generation, classify the closed outcome.
/// `up_to_date` writes nothing. A `fast_forward` and a diverged merge both go
/// through `publish_merge` (R007), which plans the write and answers a
/// [`publish::MergeStep`]: the router arm applies the plan in one transaction.
pub async fn plan_branch_merge(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: BranchMergeRequest,
) -> Result<publish::MergeStep, Response> {
    if request.tenant != verified.tenant() {
        return Err(merge_error_response(
            req_id,
            MergeClassifyError::Refused(ServerErrorCode::BranchTenantDenied),
        ));
    }
    let registry = {
        let s = state.read().await;
        shared_branch_registry(&s).map_err(|error| Response::err(req_id, error))?
    };
    let reference = |name: &str| BranchRef {
        tenant: request.tenant.clone(),
        graph: request.graph.clone(),
        name: name.to_string(),
    };
    let (source, target) = (reference(&request.source), reference(&request.target));
    let tenant = verified.tenant().to_string();
    let pinned = request.merge_base.clone();
    let classified = {
        let (source, target) = (source.clone(), target.clone());
        tokio::task::spawn_blocking(move || {
            classify_branches(&registry, &source, &target, &tenant, pinned.as_deref())
        })
        .await
        .map_err(|error| Response::err(req_id, error.to_string()))?
        .map_err(|error| merge_error_response(req_id, error))?
    };
    match classified.outcome {
        MergeOutcome::UpToDate => {
            // A merge already published for this source head answers with its
            // original receipt (MEDULLA-BRANCH-MERGE-R007 replay).
            let chain = publish::chain_for_replay(state, verified, target)
                .await
                .map_err(|error| publish_error_response(req_id, error))?;
            let receipt = publish::replayed_receipt(&chain, &classified.source_head)
                .unwrap_or_else(|| plain_receipt(classified));
            Ok(publish::MergeStep::Done(receipt))
        }
        MergeOutcome::FastForward | MergeOutcome::Merged | MergeOutcome::Conflict => {
            publish::publish_merge(state, verified, source, target, &classified, request.policy)
                .await
                .map_err(|error| publish_error_response(req_id, error))
        }
    }
}

/// The served response of a merge receipt.
pub fn merge_receipt_response(req_id: u64, receipt: BranchMergeReceipt) -> Response {
    match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BranchMerge>(
        receipt,
    ) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The refusal response of a failed merge publication.
pub fn publish_failure_response(req_id: u64, error: publish::PublishError) -> Response {
    publish_error_response(req_id, error)
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
    ) -> Result<Option<medulla_mutation_types::branch::BranchCommit>, String> {
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

    /// Every commit of the branch, oldest first (R006).
    pub async fn chain(&self) -> Result<Vec<medulla_mutation_types::branch::BranchCommit>, String> {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        let mut commits = tokio::task::spawn_blocking(move || registry.chain(&reference))
            .await
            .map_err(|error| error.to_string())??;
        commits.reverse();
        Ok(commits)
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

    /// The path grants of `principal` on `repo` (MEDULLA-SCM-R007).
    pub async fn path_grants(
        &self,
        tenant: &str,
        repo: &str,
        principal: &str,
    ) -> Result<Vec<medulla_source_types::source_path_grant::PathGrant>, String> {
        let registry = Arc::clone(&self.registry);
        let (tenant, repo, principal) = (tenant.to_string(), repo.to_string(), principal.to_string());
        tokio::task::spawn_blocking(move || registry.path_grants(&tenant, &repo, &principal))
            .await
            .map_err(|error| error.to_string())?
    }

    /// Run `read` against the registry and this branch on a blocking thread
    /// (MEDULLA-SCM-R004): a read that walks the chain does so with sync calls.
    pub async fn read_blocking<R, F>(&self, read: F) -> Result<R, String>
    where
        R: Send + 'static,
        F: FnOnce(&BranchRegistry, &BranchRef) -> R + Send + 'static,
    {
        let registry = Arc::clone(&self.registry);
        let reference = self.reference.clone();
        tokio::task::spawn_blocking(move || read(&registry, &reference))
            .await
            .map_err(|error| error.to_string())
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
        F: FnOnce(&str, PayloadReader<'_>, HistoryReader<'_>) -> Result<CommitPlan, String>
            + Send
            + 'static,
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

    /// Publish one merge into this branch (MEDULLA-BRANCH-MERGE-R007), in one
    /// registry transaction: a two-parent commit or a fast-forward head move.
    pub async fn publish_merge(
        &self,
        source: &BranchRef,
        tenant: &str,
        expected_target_head: &str,
        write: MergeWrite,
    ) -> Result<AppendedCommit, HeadMoveError> {
        let registry = Arc::clone(&self.registry);
        let target = self.reference.clone();
        let source = source.clone();
        let tenant = tenant.to_string();
        let expected = expected_target_head.to_string();
        tokio::task::spawn_blocking(move || {
            registry.publish_merge(&target, &source, &tenant, &expected, &write)
        })
        .await
        .map_err(|error| HeadMoveError::Store(error.to_string()))?
    }

    /// Whether the branch head is still `expected` (the stale check before the
    /// data graph is touched).
    pub async fn head_is(&self, tenant: &str, expected: &str) -> Result<bool, String> {
        let tenant = tenant.to_string();
        let expected = expected.to_string();
        self.read_blocking(move |registry, reference| {
            registry
                .merge_view(reference, &tenant)
                .map(|view| view.map(|view| view.head == expected).unwrap_or(false))
        })
        .await?
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

/// The canonical named MessagePack of the mutation: the payload the branch
/// commit carries (MEDULLA-BRANCH-MERGE-R006) and the bytes its manifest
/// digest covers.
pub fn mutation_payload(
    mutation: &medulla_protocol_types::protocol::Method,
) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(mutation).map_err(|error| error.to_string())
}

/// SHA-256 of a mutation payload, hex: the commit manifest digest.
pub fn payload_digest(payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(payload))
}

/// The digest recorded as the commit manifest: SHA-256 of the mutation's
/// canonical named MessagePack.
pub fn mutation_manifest_digest(
    mutation: &medulla_protocol_types::protocol::Method,
) -> Result<String, String> {
    mutation_payload(mutation).map(|bytes| payload_digest(&bytes))
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

    fn view(head: &str, generation: u64, commits: &[(&str, u64, &[&str])]) -> MergeView {
        MergeView {
            head: head.to_string(),
            generation,
            commits: commits
                .iter()
                .map(|(id, generation, parents)| {
                    medulla_mutation_types::branch::BranchCommit::single_parent_commit(
                        *id,
                        "b",
                        parents.first().copied().unwrap_or_default(),
                        1,
                        "m",
                        "p",
                        0,
                    )
                    .with_generation(*generation)
                })
                .map(|mut commit| {
                    commit.parents = commits
                        .iter()
                        .find(|(id, _, _)| *id == commit.id)
                        .map(|(_, _, parents)| parents.iter().map(|p| p.to_string()).collect())
                        .unwrap_or_default();
                    commit
                })
                .collect(),
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R003
    #[test]
    fn merge_classification_covers_the_four_topologies() {
        // Linear: the target extends the source, then the reverse.
        let behind = view("g@7", 0, &[]);
        let ahead = view("c2", 2, &[("c1", 1, &["g@7"]), ("c2", 2, &["c1"])]);
        let forward = classify_merge(&ahead, &behind, None).unwrap();
        assert_eq!(forward.outcome, MergeOutcome::FastForward);
        assert_eq!(forward.merge_base, "g@7");
        let behind_source = classify_merge(&behind, &ahead, None).unwrap();
        assert_eq!(behind_source.outcome, MergeOutcome::UpToDate);

        // Equal heads publish nothing.
        let equal = classify_merge(&ahead, &ahead, None).unwrap();
        assert_eq!(equal.outcome, MergeOutcome::UpToDate);

        // Diamond: both moved from the shared base pointer.
        let left = view("s1", 1, &[("s1", 1, &["g@7"])]);
        let right = view("t1", 1, &[("t1", 1, &["g@7"])]);
        let diamond = classify_merge(&left, &right, None).unwrap();
        assert_eq!(diamond.outcome, MergeOutcome::Merged);
        assert_eq!(diamond.merge_base, "g@7");

        // A base the caller pinned that is not the computed one is stale.
        assert_eq!(
            classify_merge(&left, &right, Some("g@6")),
            Err(MergeClassifyError::Refused(ServerErrorCode::MergeTargetStale))
        );

        // Disjoint chains never get an implicit base.
        let other = view("u1", 1, &[("u1", 1, &["g@3"])]);
        assert_eq!(
            classify_merge(&left, &other, None),
            Err(MergeClassifyError::Refused(
                ServerErrorCode::BranchNoCommonAncestor
            ))
        );
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
