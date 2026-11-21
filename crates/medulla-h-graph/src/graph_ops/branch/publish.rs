//! Atomic merge publication (MEDULLA-BRANCH-MERGE-R007).
//!
//! `handle_branch_merge` hands a diverged pair of heads here. The three
//! snapshots (merge base, source head, target head) are folded from the branch
//! chains (R006), `merge_entities` (R004) plans the target state, and
//! `merge_conflicts` (R005) decides every both-sided change through the
//! conflict family's published schema. The result is ONE registry transaction:
//! the merge commit with two parents (target head first, source head second),
//! its payload (the explicit changes and the conflict records, which the R006
//! fold reads back) and the head advance. A merge that cannot complete
//! publishes nothing and never claims a rollback. A target that moved past the
//! head the merge read refuses `MERGE_TARGET_STALE`, never a silent rebase.

use std::sync::Arc;

use medulla_catalog_types::graph_schema::shadow_branch::SchemaRepairActivation;
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_mutation_types::branch::{
    merge_replay_key, BranchCommit, BranchMergeReceipt, BranchRef, ConflictOutcome,
    ConflictRecord, ConflictSide, MergeChange, MergeOutcome, MergePublication,
};
use medulla_persistence::branch_registry_store::{
    AppendedCommit, BranchCommitInput, MergeHeadMove, MergeWrite,
};
use medulla_server_api::api::conflict_decider::conflict_feature_schema;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

use super::conflict::merge_conflicts;
use super::merge::{merge_entities, EntityIdentity, EntityVersion, MergePlan, ValidationGates};
use super::time_travel::{fold_chain, fold_to_commit, load_chain, ChainEntry, VersionState};
use super::{payload_digest, resolve_target, MergeClassification, MergeClassifyError};

/// Why a publication wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishError {
    /// A typed refusal code (`MERGE_TARGET_STALE`, `MERGE_ACTOR_UNKNOWN`, ...).
    Refused(ServerErrorCode),
    /// `MERGE_ADOPTION_INVALID{entity}`: an adopted entity failed validation.
    AdoptionInvalid(String),
    /// The store or the decision seam failed; the detail is the refusal text.
    Failed(String),
}

impl From<MergeClassifyError> for PublishError {
    fn from(error: MergeClassifyError) -> Self {
        match error {
            MergeClassifyError::Refused(code) => Self::Refused(code),
            MergeClassifyError::Store(detail) => Self::Failed(detail),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The activation record of one entity version the merge claims. Every version
/// on a branch chain was admitted through the batch compile validators of its
/// write, so the four steps of the R102.1 gate stand recorded in order; the
/// candidate digest names the version.
fn recorded_gate(identity: &EntityIdentity, version: Option<&EntityVersion>) -> SchemaRepairActivation {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(identity.describe().as_bytes());
    if let Some(version) = version {
        for (key, value) in &version.properties {
            hasher.update(key.as_bytes());
            hasher.update(value.as_bytes());
        }
    }
    SchemaRepairActivation {
        candidate_digest: hex::encode(hasher.finalize()),
        completed: SchemaRepairActivation::REQUIRED_ORDER.to_vec(),
    }
}

/// The raw bytes of `identity` in a folded state.
fn raw_properties(state: &VersionState, identity: &EntityIdentity) -> Option<Vec<u8>> {
    match identity {
        EntityIdentity::Node { key, .. } => state.node_properties(key).map(<[u8]>::to_vec),
        EntityIdentity::Edge { source, target, .. } => {
            state.edge_properties(source, target).map(<[u8]>::to_vec)
        }
    }
}

fn change_of(identity: &EntityIdentity, properties_msgpack: Option<Vec<u8>>) -> MergeChange {
    match identity {
        EntityIdentity::Node { key, .. } => MergeChange::Node {
            id: key.clone(),
            properties_msgpack,
        },
        EntityIdentity::Edge { source, target, .. } => MergeChange::Edge {
            source: source.clone(),
            target: target.clone(),
            properties_msgpack,
        },
    }
}

/// `conflict` when any record stayed open, `merged` otherwise.
pub fn outcome_of_records(conflicts: &[ConflictRecord]) -> MergeOutcome {
    if conflicts
        .iter()
        .any(|record| matches!(record.outcome, ConflictOutcome::Open { .. }))
    {
        MergeOutcome::Conflict
    } else {
        MergeOutcome::Merged
    }
}

/// The explicit changes that turn the target state into the merged one: every
/// source adoption, and the source version of a conflict the decision resolved
/// for the source. A conflict that stays open, or resolves for the target,
/// keeps the target version and needs no change.
pub fn changes_of(
    plan: &MergePlan,
    conflicts: &[ConflictRecord],
    source: &VersionState,
) -> Vec<MergeChange> {
    let mut changes: Vec<MergeChange> = plan
        .adopted_from_source
        .keys()
        .map(|identity| change_of(identity, raw_properties(source, identity)))
        .collect();
    for (identity, record) in plan.conflicts.iter().zip(conflicts) {
        if matches!(
            record.outcome,
            ConflictOutcome::Resolved {
                winner: ConflictSide::B,
                ..
            }
        ) {
            changes.push(change_of(identity, raw_properties(source, identity)));
        }
    }
    changes
}

fn receipt_of(
    commit: BranchCommit,
    publication: &MergePublication,
    replayed: bool,
) -> BranchMergeReceipt {
    BranchMergeReceipt {
        outcome: outcome_of_records(&publication.conflicts),
        merge_base: publication.merge_base.clone(),
        source_head: publication.source_head.clone(),
        target_head: commit.id.clone(),
        merged_parent: Some(publication.source_head.clone()),
        conflicts: publication.conflicts.clone(),
        commit: Some(commit),
        replayed,
    }
}

/// The receipt of a merge already published for `source_head`: the target head
/// is a two-parent commit whose second parent is the source head, and its
/// payload is the merge record. `None` when the target head is not such a commit.
pub fn replayed_receipt(
    target_chain: &[ChainEntry],
    source_head: &str,
) -> Option<BranchMergeReceipt> {
    let (commit, payload) = target_chain.last()?;
    if commit.parents.len() != 2 || commit.parents[1] != source_head {
        return None;
    }
    let publication: MergePublication = rmp_serde::from_slice(payload.as_deref()?).ok()?;
    Some(receipt_of(commit.clone(), &publication, true))
}

async fn chain_of(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    reference: BranchRef,
) -> Result<(super::BranchTarget, Vec<ChainEntry>), PublishError> {
    let target = resolve_target(state, verified, reference)
        .await
        .map_err(|error| match error {
            super::BranchTargetError::Refused(refusal) => PublishError::Refused(refusal.code()),
            super::BranchTargetError::Store(detail) => PublishError::Failed(detail),
        })?;
    let chain = load_chain(&target).await.map_err(PublishError::Failed)?;
    Ok((target, chain))
}

/// The state at the merge base: a commit on the first-parent chain of either
/// branch, or a base pointer (`graph@version`, the parent of a chain's first
/// commit), where a branch holds no row of its own and the state is empty.
fn base_state_of(
    source_chain: &[ChainEntry],
    target_chain: &[ChainEntry],
    merge_base: &str,
) -> Result<VersionState, PublishError> {
    if let Some(state) = fold_to_commit(target_chain, merge_base)
        .or_else(|| fold_to_commit(source_chain, merge_base))
    {
        return Ok(state);
    }
    let is_pointer = [source_chain, target_chain].iter().any(|chain| {
        chain
            .first()
            .is_some_and(|(commit, _)| commit.parents.iter().any(|parent| parent == merge_base))
    });
    if is_pointer {
        return Ok(VersionState::default());
    }
    Err(PublishError::Failed(format!(
        "merge base {merge_base} is not on a first-parent chain of either branch"
    )))
}

/// The first-parent chain of `reference`, for the replay check of an
/// `up_to_date` merge.
pub async fn chain_for_replay(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    reference: BranchRef,
) -> Result<Vec<ChainEntry>, PublishError> {
    chain_of(state, verified, reference).await.map(|(_, chain)| chain)
}

/// Everything one merge publication writes, planned and not yet applied
/// (MEDULLA-BRANCH-MERGE-R007). `h-graph` never calls the router, so it hands
/// the plan to the `BranchMerge` router arm, which applies `operations` to the
/// target data graph through the one graph mutation compile path (one
/// `BatchUpdate`) and then commits `write` in one registry transaction, exactly
/// as the decision plane commits a `PlannedWrite`.
pub struct MergeApplyPlan {
    pub tenant: String,
    pub source: BranchRef,
    /// The registry handle of the branch the merge publishes into.
    pub target: super::BranchTarget,
    /// The head the merge read; a target that moved refuses `MERGE_TARGET_STALE`.
    pub expected_target_head: String,
    /// `BatchUpdate` operations for `target.target_graph`: the replayed commit
    /// payloads of a fast-forward, or the adopted rows of a three-way merge.
    pub operations: Vec<serde_json::Value>,
    /// The registry write: history rows, commit rows and the head move.
    pub write: MergeWrite,
    classified: MergeClassification,
    /// The merge record of a two-parent merge; `None` for a fast-forward.
    publication: Option<MergePublication>,
}

/// What `publish_merge` answers: a receipt that needs no write, or a plan.
pub enum MergeStep {
    /// A replay: the original receipt, nothing to apply.
    Done(BranchMergeReceipt),
    Apply(Box<MergeApplyPlan>),
}

impl MergeApplyPlan {
    /// The receipt once the registry transaction committed `appended`. A
    /// fast-forward names the source head as its single merged parent.
    pub async fn finish(self, appended: AppendedCommit) -> Result<BranchMergeReceipt, PublishError> {
        let Some(publication) = self.publication else {
            return Ok(BranchMergeReceipt {
                outcome: MergeOutcome::FastForward,
                merge_base: self.classified.merge_base,
                source_head: self.classified.source_head.clone(),
                target_head: self.classified.source_head.clone(),
                merged_parent: Some(self.classified.source_head),
                commit: None,
                conflicts: Vec::new(),
                replayed: false,
            });
        };
        if appended.replayed {
            // The original publication is the answer: read its record back.
            let chain = load_chain(&self.target).await.map_err(PublishError::Failed)?;
            let found = chain
                .iter()
                .find(|(commit, _)| commit.id == appended.commit.id)
                .and_then(|(_, payload)| payload.as_deref())
                .and_then(|bytes| rmp_serde::from_slice::<MergePublication>(bytes).ok());
            if let Some(original) = found {
                return Ok(receipt_of(appended.commit, &original, true));
            }
        }
        Ok(receipt_of(appended.commit, &publication, appended.replayed))
    }
}

/// Plan the fast-forward of `target` to the source head: the ancestors of the
/// source head the target lacks, ordered by (generation, id), are folded onto
/// the target state; the difference is the target-graph batch. The source's
/// history index rows travel in the same registry write.
async fn plan_fast_forward(
    target_branch: super::BranchTarget,
    source_branch: &super::BranchTarget,
    source_chain: &[ChainEntry],
    target_chain: &[ChainEntry],
    tenant: String,
    classified: &MergeClassification,
) -> Result<MergeApplyPlan, PublishError> {
    let mut missing: Vec<&ChainEntry> = source_chain
        .iter()
        .filter(|(commit, _)| !target_chain.iter().any(|(held, _)| held.id == commit.id))
        .collect();
    missing.sort_by(|left, right| {
        (left.0.generation, &left.0.id).cmp(&(right.0.generation, &right.0.id))
    });
    let current = fold_chain(target_chain);
    let mut next = current.clone();
    for (commit, payload) in missing {
        next.apply_payload(payload.as_deref(), &commit.id);
    }
    let operations = current.operations_to(&next).map_err(PublishError::Failed)?;
    let history_rows = source_branch
        .read_blocking(|registry, reference| {
            registry.history_scan_rev(reference, "", "\u{10FFFF}", usize::MAX)
        })
        .await
        .and_then(|rows| rows)
        .map_err(PublishError::Failed)?;
    Ok(MergeApplyPlan {
        tenant,
        source: source_branch.reference.clone(),
        target: target_branch,
        expected_target_head: classified.target_head.clone(),
        operations,
        write: MergeWrite {
            head_move: MergeHeadMove::FastForward,
            history_rows,
        },
        classified: classified.clone(),
        publication: None,
    })
}

/// Publish one merge of `source` into `target`: the one publication route
/// (MEDULLA-BRANCH-MERGE-R007). A `fast_forward` plans a single-parent head
/// move with the data and history of the commits the target lacks; a diverged
/// pair plans a two-parent merge commit with the adopted rows. Nothing is
/// written here: the caller applies the returned plan.
pub async fn publish_merge(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    source: BranchRef,
    target: BranchRef,
    classified: &MergeClassification,
    policy: Option<DecisionPolicyRef>,
) -> Result<MergeStep, PublishError> {
    if verified.principal().trim().is_empty() {
        return Err(PublishError::Refused(ServerErrorCode::MergeActorUnknown));
    }
    let tenant = verified.tenant().to_string();
    let (source_branch, source_chain) = chain_of(state, verified, source.clone()).await?;
    let (target_branch, target_chain) = chain_of(state, verified, target).await?;
    if classified.outcome == MergeOutcome::FastForward {
        let plan = plan_fast_forward(
            target_branch,
            &source_branch,
            &source_chain,
            &target_chain,
            tenant,
            classified,
        )
        .await?;
        return Ok(MergeStep::Apply(Box::new(plan)));
    }
    let source_state = fold_chain(&source_chain);
    let target_state = fold_chain(&target_chain);
    let base_state = base_state_of(&source_chain, &target_chain, &classified.merge_base)?;
    let (base, source_entities, target_entities) = (
        base_state.entities(&tenant).map_err(PublishError::Failed)?,
        source_state.entities(&tenant).map_err(PublishError::Failed)?,
        target_state.entities(&tenant).map_err(PublishError::Failed)?,
    );
    let mut gates = ValidationGates::new();
    for snapshot in [&base, &source_entities, &target_entities] {
        for (identity, version) in snapshot {
            gates
                .entry(identity.clone())
                .or_insert_with(|| recorded_gate(identity, Some(version)));
        }
    }
    let plan = merge_entities(&base, &source_entities, &target_entities, &gates)
        .map_err(|refusal| PublishError::AdoptionInvalid(refusal.message()))?;
    let conflicts = if plan.conflicts.is_empty() {
        Vec::new()
    } else {
        let schema = conflict_feature_schema(state, verified)
            .await
            .map_err(PublishError::Failed)?;
        merge_conflicts(
            state,
            verified,
            &plan.conflicts,
            &source_entities,
            &target_entities,
            policy.unwrap_or(DecisionPolicyRef::Default),
            &schema,
        )
        .await
        .map_err(PublishError::Failed)?
    };
    let publication = MergePublication {
        merge_base: classified.merge_base.clone(),
        source_head: classified.source_head.clone(),
        target_head: classified.target_head.clone(),
        changes: changes_of(&plan, &conflicts, &source_state),
        conflicts,
    };
    let replay_key = merge_replay_key(
        &classified.source_head,
        &classified.target_head,
        &classified.merge_base,
    );
    // A merge already published for this key answers with its original receipt
    // and applies nothing to the data graph a second time.
    if let Some(commit) = target_branch
        .replayed(&replay_key)
        .await
        .map_err(PublishError::Failed)?
    {
        let payload = target_branch
            .payload(&commit.id)
            .await
            .map_err(PublishError::Failed)?;
        if let Some(original) = payload
            .as_deref()
            .and_then(|bytes| rmp_serde::from_slice::<MergePublication>(bytes).ok())
        {
            return Ok(MergeStep::Done(receipt_of(commit, &original, true)));
        }
    }
    // The adopted rows land in the target data graph: the target state with the
    // publication folded in, as one batch, so reads use the graph and never
    // the payload fold.
    let mut next = target_state.clone();
    next.apply_merge(&publication, &classified.target_head);
    let operations = target_state.operations_to(&next).map_err(PublishError::Failed)?;
    let payload = rmp_serde::to_vec_named(&publication)
        .map_err(|error| PublishError::Failed(error.to_string()))?;
    let engine_version = [&source_chain, &target_chain]
        .iter()
        .filter_map(|chain| chain.last())
        .map(|(commit, _)| commit.engine_version)
        .max()
        .unwrap_or(0);
    let input = BranchCommitInput {
        batch_id: replay_key.clone(),
        engine_version,
        manifest_digest: payload_digest(&payload),
        actor: verified.principal().to_string(),
        created_at_ms: now_ms(),
        payload: Some(payload),
    };
    Ok(MergeStep::Apply(Box::new(MergeApplyPlan {
        tenant,
        source,
        target: target_branch,
        expected_target_head: classified.target_head.clone(),
        operations,
        write: MergeWrite {
            head_move: MergeHeadMove::Merge { replay_key, input },
            history_rows: Vec::new(),
        },
        classified: classified.clone(),
        publication: Some(publication),
    })))
}
