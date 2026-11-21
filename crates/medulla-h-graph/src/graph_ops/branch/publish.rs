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

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_catalog_types::graph_schema::shadow_branch::SchemaRepairActivation;
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_mutation_types::branch::{
    merge_replay_key, BranchCommit, BranchMergeReceipt, BranchRef, ConflictOutcome,
    ConflictRecord, ConflictSide, MergeChange, MergeOutcome, MergePublication,
};
use medulla_persistence::branch_registry_store::{
    AppendedCommit, BranchCommitInput, HeadMoveError, MergeHeadMove, MergeWrite,
};
use medulla_server_api::api::conflict_decider::conflict_feature_schema;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

use super::conflict::merge_conflicts;
use super::merge::{
    merge_entities, EntityIdentity, EntitySnapshot, EntityVersion, MergePlan, ValidationGates,
};
use super::time_travel::{fold_chain, fold_to_commit, load_chain, ChainEntry, VersionState};
use super::{payload_digest, resolve_target, MergeClassification, MergeClassifyError};

/// Why a publication wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishError {
    /// A typed refusal code (`MERGE_TARGET_STALE`, `MERGE_ACTOR_UNKNOWN`, ...).
    Refused(ServerErrorCode),
    /// `MERGE_ADOPTION_INVALID{entity}`: an adopted entity failed validation.
    AdoptionInvalid(String),
    /// `BRANCH_PAYLOAD_UNREPLAYABLE`: a branch carries payload methods the merge cannot fold
    /// into rows (their names, comma separated). The merge publishes nothing.
    Unreplayable(String),
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

/// Refuse a merge when any folded commit carries a payload method that has no row effect the
/// merge can replay: publishing the folded state would leave that effect out.
fn require_replayable(states: &[&VersionState]) -> Result<(), PublishError> {
    let mut methods: Vec<&str> = states
        .iter()
        .flat_map(|state| state.unreplayable().iter().map(String::as_str))
        .collect();
    methods.sort_unstable();
    methods.dedup();
    if methods.is_empty() {
        Ok(())
    } else {
        Err(PublishError::Unreplayable(methods.join(", ")))
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

/// The registry half of a merge: the branch that takes the head move, the head the merge read and
/// the transaction to commit.
#[derive(Clone)]
pub struct RegistryStep {
    /// The registry handle of the branch the merge publishes into.
    pub target: super::BranchTarget,
    /// The head the merge read; a target that moved refuses `MERGE_TARGET_STALE`.
    pub expected_target_head: String,
    /// The registry write: history rows, commit rows and the head move.
    pub write: MergeWrite,
}

/// Everything one merge publication writes, planned and not yet applied
/// (MEDULLA-BRANCH-MERGE-R007). `h-graph` never calls the router, so it hands
/// the plan to the `BranchMerge` router arm, which commits it through
/// [`MergeApplyPlan::commit`]: `operations` land in the target data graph through the one graph
/// mutation compile path (one `BatchUpdate`), then `registry` commits in one registry
/// transaction, exactly as the decision plane commits a `PlannedWrite`.
#[derive(Clone)]
pub struct MergeApplyPlan {
    pub tenant: String,
    pub source: BranchRef,
    /// The graph the operations land in: the target branch's own graph, or the base graph when
    /// the target is `main`.
    pub target_graph: String,
    /// `BatchUpdate` operations for `target_graph`: the replayed commit payloads of a
    /// fast-forward, the adopted rows of a three-way merge, or the net changes of a branch
    /// merged into `main`. Applying them twice leaves the same state.
    pub operations: Vec<serde_json::Value>,
    /// The registry half; `None` when the target is `main`, which has no registry row.
    pub registry: Option<RegistryStep>,
    classified: MergeClassification,
    /// The merge record of a two-parent merge; `None` for a fast-forward.
    publication: Option<MergePublication>,
    /// The conflict records of a merge into `main`, which has no commit payload to carry them;
    /// empty for every other merge.
    trunk_conflicts: Vec<ConflictRecord>,
}

/// What `publish_merge` answers: a receipt that needs no write, or a plan.
pub enum MergeStep {
    /// A replay: the original receipt, nothing to apply.
    Done(BranchMergeReceipt),
    Apply(Box<MergeApplyPlan>),
}

/// Why a merge commit wrote or finished nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeCommitError {
    /// The graph batch was refused; the text is the refusal.
    Graph(String),
    /// The registry transaction refused or failed; `Stale` is `MERGE_TARGET_STALE`.
    Head(HeadMoveError),
    /// The receipt could not be built.
    Finish(PublishError),
    /// A registry read failed.
    Store(String),
}

/// The two writes of a merge. The router implements `apply_graph` through the graph mutation
/// compile path; `publish` is the registry transaction and a test replaces it to fail one.
#[allow(async_fn_in_trait)]
pub trait MergeEffects {
    /// Apply `operations` to `graph` as one batch, creating the graph first when `create_graph`.
    /// The operations are full replacements and removals, so a repeat is harmless.
    async fn apply_graph(
        &self,
        graph: &str,
        operations: &[serde_json::Value],
        create_graph: bool,
    ) -> Result<(), String>;

    /// Commit the registry half in one transaction.
    async fn publish(
        &self,
        step: &RegistryStep,
        source: &BranchRef,
        tenant: &str,
    ) -> Result<AppendedCommit, HeadMoveError> {
        step.target
            .publish_merge(source, tenant, &step.expected_target_head, step.write.clone())
            .await
    }
}

impl MergeApplyPlan {
    /// Commit the plan, retry-safe by construction: a stale target head is refused before
    /// anything is written, the graph batch commits first, then the registry transaction under
    /// the merge's replay key. When the registry write fails after the graph batch committed,
    /// nothing is claimed rolled back: the same merge run again re-reads the same heads, plans
    /// the same operations, applies them again over the same rows and commits the head move,
    /// and a merge whose registry write did commit answers its original receipt.
    pub async fn commit<E: MergeEffects>(
        self,
        effects: &E,
    ) -> Result<BranchMergeReceipt, MergeCommitError> {
        if let Some(step) = &self.registry {
            match step
                .target
                .head_is(&self.tenant, &step.expected_target_head)
                .await
            {
                Ok(true) => {}
                Ok(false) => return Err(MergeCommitError::Head(HeadMoveError::Stale)),
                Err(error) => return Err(MergeCommitError::Store(error)),
            }
        }
        if !self.operations.is_empty() {
            effects
                .apply_graph(&self.target_graph, &self.operations, self.registry.is_some())
                .await
                .map_err(MergeCommitError::Graph)?;
        }
        let appended = match &self.registry {
            Some(step) => Some(
                effects
                    .publish(step, &self.source, &self.tenant)
                    .await
                    .map_err(MergeCommitError::Head)?,
            ),
            None => None,
        };
        self.finish(appended).await.map_err(MergeCommitError::Finish)
    }

    /// The receipt once the registry transaction committed `appended` (`None` for a merge into
    /// `main`, which has no registry row). A fast-forward names the source head as its single
    /// merged parent.
    async fn finish(
        self,
        appended: Option<AppendedCommit>,
    ) -> Result<BranchMergeReceipt, PublishError> {
        let Some(publication) = self.publication else {
            let outcome = if self.trunk_conflicts.is_empty() {
                MergeOutcome::FastForward
            } else {
                outcome_of_records(&self.trunk_conflicts)
            };
            return Ok(BranchMergeReceipt {
                outcome,
                merge_base: self.classified.merge_base,
                source_head: self.classified.source_head.clone(),
                target_head: self.classified.source_head.clone(),
                merged_parent: Some(self.classified.source_head),
                commit: None,
                conflicts: self.trunk_conflicts,
                replayed: false,
            });
        };
        let (Some(appended), Some(step)) = (appended, &self.registry) else {
            return Err(PublishError::Failed(
                "a two-parent merge has no registry commit".to_string(),
            ));
        };
        if appended.replayed {
            // The original publication is the answer: read its record back.
            let chain = load_chain(&step.target).await.map_err(PublishError::Failed)?;
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
        target_graph: target_branch.target_graph.clone(),
        operations,
        registry: Some(RegistryStep {
            target: target_branch,
            expected_target_head: classified.target_head.clone(),
            write: MergeWrite {
                head_move: MergeHeadMove::FastForward,
                history_rows,
            },
        }),
        classified: classified.clone(),
        publication: None,
        trunk_conflicts: Vec::new(),
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
    require_replayable(&[&fold_chain(&source_chain), &fold_chain(&target_chain)])?;
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
        target_graph: target_branch.target_graph.clone(),
        operations,
        registry: Some(RegistryStep {
            target: target_branch,
            expected_target_head: classified.target_head.clone(),
            write: MergeWrite {
                head_move: MergeHeadMove::Merge { replay_key, input },
                history_rows: Vec::new(),
            },
        }),
        classified: classified.clone(),
        publication: Some(publication),
        trunk_conflicts: Vec::new(),
    })))
}

/// The version a branch was cut at, read from its base pointer `<graph>@<version>`.
fn base_version_of(merge_base: &str) -> Result<u64, PublishError> {
    merge_base
        .rsplit_once('@')
        .and_then(|(_, version)| version.parse().ok())
        .ok_or_else(|| {
            PublishError::Failed(format!("merge base {merge_base} is not a base pointer"))
        })
}

/// The node rows of the base graph `graph` for `ids`: as of `base_version` (what the branch was
/// cut from) and as they stand now (what `main` holds). The graph keeps a version-indexed image
/// per node, so a node `main` changed after the cut differs between the two.
async fn trunk_node_rows(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    base_version: u64,
    ids: &std::collections::BTreeSet<String>,
) -> Result<(BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>), PublishError> {
    let s = state.read().await;
    let entry = s
        .registry
        .get(graph)
        .ok_or_else(|| PublishError::Failed(format!("graph {graph} is not loaded")))?;
    let (mut base, mut current) = (BTreeMap::new(), BTreeMap::new());
    for id in ids {
        let at_base = entry
            .core
            .node_properties_at(id, base_version)
            .map_err(|error| PublishError::Failed(error.to_string()))?;
        if let Some(bytes) = at_base {
            base.insert(id.clone(), bytes.as_ref().clone());
        }
        if let Some(bytes) = entry.core.get_node_properties(id) {
            current.insert(id.clone(), bytes);
        }
    }
    Ok((base, current))
}

/// The three-way classification of a branch's node writes against `main`: the base is the base
/// graph's state at the version the branch was cut at, the target is the base graph now, the
/// source is the branch's net write. A node changed on both sides is a conflict; a node changed
/// on one side is adopted or held. Only nodes the branch wrote or removed take part: the base
/// graph keeps no version-indexed edge images, so an edge the branch wrote is adopted.
fn trunk_three_way(
    tenant: &str,
    source_state: &VersionState,
    base_rows: BTreeMap<String, Vec<u8>>,
    target_rows: BTreeMap<String, Vec<u8>>,
) -> Result<(MergePlan, EntitySnapshot, EntitySnapshot), PublishError> {
    let nodes_only = |snapshot: EntitySnapshot| -> EntitySnapshot {
        snapshot
            .into_iter()
            .filter(|(identity, _)| matches!(identity, EntityIdentity::Node { .. }))
            .collect()
    };
    let entities = |state: &VersionState| {
        state
            .entities(tenant)
            .map(nodes_only)
            .map_err(PublishError::Failed)
    };
    let base = entities(&VersionState::from_node_rows(base_rows))?;
    let target = entities(&VersionState::from_node_rows(target_rows))?;
    let source = entities(source_state)?;
    let mut gates = ValidationGates::new();
    for snapshot in [&base, &source, &target] {
        for (identity, version) in snapshot {
            gates
                .entry(identity.clone())
                .or_insert_with(|| recorded_gate(identity, Some(version)));
        }
    }
    let plan = merge_entities(&base, &source, &target, &gates)
        .map_err(|refusal| PublishError::AdoptionInvalid(refusal.message()))?;
    Ok((plan, source, target))
}

/// The branch's net operations for the base graph, less the node writes the merge did not
/// adopt: a node `main` changed (held, or a conflict that stayed open or resolved for `main`)
/// keeps its `main` version.
fn trunk_batch(
    operations: Vec<serde_json::Value>,
    touched: &std::collections::BTreeSet<String>,
    plan: &MergePlan,
    conflicts: &[ConflictRecord],
) -> Vec<serde_json::Value> {
    let node_key = |identity: &EntityIdentity| match identity {
        EntityIdentity::Node { key, .. } => Some(key.clone()),
        EntityIdentity::Edge { .. } => None,
    };
    let mut allowed: std::collections::BTreeSet<String> =
        plan.adopted_from_source.keys().filter_map(node_key).collect();
    for (identity, record) in plan.conflicts.iter().zip(conflicts) {
        if matches!(
            record.outcome,
            ConflictOutcome::Resolved {
                winner: ConflictSide::B,
                ..
            }
        ) {
            allowed.extend(node_key(identity));
        }
    }
    operations
        .into_iter()
        .filter(|operation| {
            let is_node_write = matches!(
                operation["op"].as_str(),
                Some("add_node") | Some("remove_node")
            );
            let id = operation["id"].as_str().unwrap_or_default();
            !(is_node_write && touched.contains(id) && !allowed.contains(id))
        })
        .collect()
}

/// Plan the merge of `source` into `main`, the base graph `graph` (MEDULLA-BRANCH-MERGE-R007).
///
/// `main` is not a registry branch: it has no commit chain, head or registry transaction, so the
/// plan carries only the graph batch. The batch is the net change of the source chain (every
/// removal it made and every row it leaves, [`VersionState::trunk_operations`]) written over the
/// base graph, less the nodes `main` changed since the branch was cut. Those are found against
/// the base graph's node image at the branch's base version ([`trunk_three_way`]); a node changed
/// on both sides goes through the conflict decider exactly like a branch merge, and an open
/// conflict leaves the `main` version. Silent overwriting never happens. The receipt names the
/// source head as the merged parent and carries the conflict records.
pub async fn plan_trunk_merge(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    source: BranchRef,
    graph: &str,
    policy: Option<DecisionPolicyRef>,
) -> Result<MergeStep, PublishError> {
    if verified.principal().trim().is_empty() {
        return Err(PublishError::Refused(ServerErrorCode::MergeActorUnknown));
    }
    let (source_branch, source_chain) = chain_of(state, verified, source.clone()).await?;
    let source_state = fold_chain(&source_chain);
    require_replayable(&[&source_state])?;
    let tenant = verified.tenant().to_string();
    let head = {
        let tenant = tenant.clone();
        source_branch
            .read_blocking(move |registry, reference| registry.merge_view(reference, &tenant))
            .await
            .and_then(|view| view)
            .map_err(PublishError::Failed)?
            .map_err(|refusal| PublishError::Refused(refusal.code()))?
            .head
    };
    let merge_base = source_chain
        .first()
        .and_then(|(commit, _)| commit.parents.first().cloned())
        .unwrap_or_else(|| head.clone());
    let classified = MergeClassification {
        outcome: MergeOutcome::FastForward,
        merge_base,
        source_head: head.clone(),
        target_head: head.clone(),
    };
    let mut operations = source_state.trunk_operations().map_err(PublishError::Failed)?;
    let touched = source_state.touched_nodes();
    let mut conflicts = Vec::new();
    if !touched.is_empty() {
        let base_version = base_version_of(&classified.merge_base)?;
        let (base_rows, target_rows) =
            trunk_node_rows(state, graph, base_version, &touched).await?;
        let (plan, source_entities, target_entities) =
            trunk_three_way(&tenant, &source_state, base_rows, target_rows)?;
        if !plan.conflicts.is_empty() {
            let schema = conflict_feature_schema(state, verified)
                .await
                .map_err(PublishError::Failed)?;
            conflicts = merge_conflicts(
                state,
                verified,
                &plan.conflicts,
                &source_entities,
                &target_entities,
                policy.unwrap_or(DecisionPolicyRef::Default),
                &schema,
            )
            .await
            .map_err(PublishError::Failed)?;
        }
        operations = trunk_batch(operations, &touched, &plan, &conflicts);
    }
    if operations.is_empty() {
        let outcome = if conflicts.is_empty() {
            MergeOutcome::UpToDate
        } else {
            outcome_of_records(&conflicts)
        };
        return Ok(MergeStep::Done(BranchMergeReceipt {
            outcome,
            merge_base: classified.merge_base,
            source_head: head.clone(),
            target_head: head,
            merged_parent: None,
            commit: None,
            conflicts,
            replayed: false,
        }));
    }
    Ok(MergeStep::Apply(Box::new(MergeApplyPlan {
        tenant,
        source,
        target_graph: graph.to_string(),
        operations,
        registry: None,
        classified,
        publication: None,
        trunk_conflicts: conflicts,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_mutation_types::branch::Branch;
    use medulla_persistence::branch_registry_store::BranchRegistry;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    const TENANT: &str = "tenant-a";

    fn row(pairs: &[(&str, &str)]) -> Vec<u8> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        rmp_serde::to_vec_named(&map).unwrap()
    }

    // spec: MEDULLA-BRANCH-MERGE-R007
    #[test]
    fn a_trunk_merge_detects_a_node_both_sides_changed_and_keeps_the_main_version() {
        let nodes: BTreeMap<String, Vec<u8>> = [
            ("both".to_string(), row(&[("type", "T"), ("v", "branch")])),
            ("branch-only".to_string(), row(&[("type", "T"), ("v", "branch")])),
        ]
        .into();
        let source = VersionState::from_node_rows(nodes);
        let base: BTreeMap<String, Vec<u8>> =
            [("both".to_string(), row(&[("type", "T"), ("v", "base")]))].into();
        let main: BTreeMap<String, Vec<u8>> = [(
            "both".to_string(),
            row(&[("type", "T"), ("v", "main")]),
        )]
        .into();
        let (plan, _, _) = trunk_three_way(TENANT, &source, base, main).unwrap();
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.adopted_from_source.len(), 1);
        let operations = source.trunk_operations().unwrap();
        let kept = trunk_batch(operations, &source.touched_nodes(), &plan, &[]);
        let ids: Vec<&str> = kept.iter().filter_map(|op| op["id"].as_str()).collect();
        assert_eq!(ids, vec!["branch-only"]);
    }

    /// A graph that applies the plan's operations as the compile path does (`add_node` replaces
    /// a row, `remove_node` deletes it) and a registry write that fails once.
    struct FlakyEffects {
        graph: Mutex<BTreeMap<String, serde_json::Value>>,
        applies: AtomicUsize,
        fail_publish_once: AtomicBool,
    }

    impl MergeEffects for FlakyEffects {
        async fn apply_graph(
            &self,
            _graph: &str,
            operations: &[serde_json::Value],
            _create_graph: bool,
        ) -> Result<(), String> {
            self.applies.fetch_add(1, Ordering::SeqCst);
            let mut graph = self.graph.lock().unwrap();
            for operation in operations {
                let id = operation["id"].as_str().unwrap().to_string();
                match operation["op"].as_str().unwrap() {
                    "add_node" => {
                        graph.insert(id, operation["properties"].clone());
                    }
                    "remove_node" => {
                        graph.remove(&id);
                    }
                    other => panic!("unexpected operation {other}"),
                }
            }
            Ok(())
        }

        async fn publish(
            &self,
            step: &RegistryStep,
            source: &BranchRef,
            tenant: &str,
        ) -> Result<AppendedCommit, HeadMoveError> {
            if self.fail_publish_once.swap(false, Ordering::SeqCst) {
                return Err(HeadMoveError::Store("registry write failed".to_string()));
            }
            step.target
                .publish_merge(source, tenant, &step.expected_target_head, step.write.clone())
                .await
        }
    }

    fn branch(registry: &BranchRegistry, name: &str) -> BranchRef {
        let reference = BranchRef {
            tenant: TENANT.into(),
            graph: "g".into(),
            name: name.into(),
        };
        registry
            .create(&Branch {
                tenant: TENANT.into(),
                graph: "g".into(),
                name: name.into(),
                base_version: 1,
                head_commit: "g@1".into(),
                created_by: "agent".into(),
                created_at: 0,
                deleted: false,
            })
            .unwrap();
        let input = BranchCommitInput {
            batch_id: format!("write-{name}"),
            engine_version: 2,
            manifest_digest: format!("digest-{name}"),
            actor: "agent".into(),
            created_at_ms: 1,
            payload: None,
        };
        registry.append_commit(&reference, TENANT, &input).unwrap();
        reference
    }

    // spec: MEDULLA-BRANCH-MERGE-R007
    #[tokio::test]
    async fn a_merge_rerun_after_the_registry_write_failed_finishes_the_head_move() {
        let dir = std::env::temp_dir().join(format!("merge-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let registry = Arc::new(BranchRegistry::open(dir.to_str().unwrap()).unwrap());
        let source = branch(&registry, "feature/source");
        let target_ref = branch(&registry, "feature/target");
        let heads = |reference: &BranchRef| registry.merge_view(reference, TENANT).unwrap().unwrap();
        let (source_head, target_head) = (heads(&source).head, heads(&target_ref).head);
        let replay_key = merge_replay_key(&source_head, &target_head, "g@1");
        let publication = MergePublication {
            merge_base: "g@1".into(),
            source_head: source_head.clone(),
            target_head: target_head.clone(),
            changes: vec![MergeChange::Node {
                id: "n".into(),
                properties_msgpack: Some(vec![0x80]),
            }],
            conflicts: Vec::new(),
        };
        let payload = rmp_serde::to_vec_named(&publication).unwrap();
        let input = BranchCommitInput {
            batch_id: replay_key.clone(),
            engine_version: 2,
            manifest_digest: payload_digest(&payload),
            actor: "agent".into(),
            created_at_ms: 3,
            payload: Some(payload),
        };
        let target = super::super::BranchTarget {
            reference: target_ref.clone(),
            target_graph: "g--feature-target".into(),
            registry: Arc::clone(&registry),
        };
        let plan = MergeApplyPlan {
            tenant: TENANT.into(),
            source: source.clone(),
            target_graph: target.target_graph.clone(),
            operations: vec![serde_json::json!({"op": "add_node", "id": "n", "properties": {}})],
            registry: Some(RegistryStep {
                target: target.clone(),
                expected_target_head: target_head.clone(),
                write: MergeWrite {
                    head_move: MergeHeadMove::Merge {
                        replay_key,
                        input,
                    },
                    history_rows: Vec::new(),
                },
            }),
            classified: MergeClassification {
                outcome: MergeOutcome::Merged,
                merge_base: "g@1".into(),
                source_head: source_head.clone(),
                target_head: target_head.clone(),
            },
            publication: Some(publication),
            trunk_conflicts: Vec::new(),
        };
        let effects = FlakyEffects {
            graph: Mutex::new(BTreeMap::new()),
            applies: AtomicUsize::new(0),
            fail_publish_once: AtomicBool::new(true),
        };

        // The registry write fails after the graph batch committed: nothing is rolled back and
        // the target head has not moved.
        let failed = plan.clone().commit(&effects).await.unwrap_err();
        assert_eq!(
            failed,
            MergeCommitError::Head(HeadMoveError::Store("registry write failed".to_string()))
        );
        assert_eq!(effects.graph.lock().unwrap().len(), 1);
        assert!(target.head_is(TENANT, &target_head).await.unwrap());

        // The same merge run again re-applies the same rows (a repeat changes nothing) and
        // finishes the head move: one two-parent commit, the target head first.
        let receipt = plan.clone().commit(&effects).await.unwrap();
        assert_eq!(receipt.outcome, MergeOutcome::Merged);
        assert!(!receipt.replayed);
        let commit = receipt.commit.expect("the merge commit");
        assert_eq!(commit.parents, vec![target_head.clone(), source_head]);
        assert_eq!(heads(&target_ref).head, commit.id);
        assert_eq!(effects.graph.lock().unwrap().len(), 1);
        assert_eq!(effects.applies.load(Ordering::SeqCst), 2);

        // A merge whose registry write committed answers its original commit and writes nothing.
        let step = plan.registry.as_ref().unwrap();
        let replayed = target
            .publish_merge(&source, TENANT, &step.expected_target_head, step.write.clone())
            .await
            .unwrap();
        assert!(replayed.replayed);
        assert_eq!(replayed.commit.id, commit.id);
        std::fs::remove_dir_all(&dir).ok();
    }
}
