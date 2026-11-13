//! The durable branch registry (MEDULLA-BRANCH-MERGE-R001).
//!
//! One kernel-owned owner file, `branch_registry.redb`, of layout
//! `OwnerLayout::BranchRegistry`. A branch is one registry row; it copies no
//! graph data. Every create is one admitted, ledgered, fenced and
//! version-bumping mutation through the store's own `MutationKernel`, and the
//! existence check runs inside that same write transaction, so two racing
//! creates of one name cannot both land.

use std::path::Path;
use std::sync::Arc;

use medulla_types::branch::{
    branch_commit_id, Branch, BranchCommit, BranchMutationRefusal, BranchRef,
};
use medulla_types::contract::ServerErrorCode;
use redb::{ReadableTable, ReadableTableMetadata};

/// The owner file name under the persist dir.
pub const BRANCH_REGISTRY_FILE: &str = "branch_registry.redb";
/// The physical authority of [`BRANCH_REGISTRY_FILE`].
pub const BRANCH_REGISTRY_PHYSICAL_STORE: &str = "epistemic-graph:branch-registry";
const BRANCH_REGISTRY_SCOPE_RESOURCE: &str = "branch-registry";
const BRANCH_REGISTRY_SCOPE_INCARNATION: &str = "branch-registry:v1";

type Durable = medulla_store::sidecar_store::SidecarStore<medulla_storage::BranchRegistryOwner>;

/// Why a create wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchCreateError {
    /// A refusal the caller can act on.
    Refused(ServerErrorCode),
    /// The store itself failed.
    Store(String),
}

impl From<String> for BranchCreateError {
    fn from(error: String) -> Self {
        Self::Store(error)
    }
}

/// The branch registry owner file.
pub struct BranchRegistry {
    durable: Durable,
}

impl std::fmt::Debug for BranchRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BranchRegistry").finish_non_exhaustive()
    }
}

fn encode(branch: &Branch) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(branch).map_err(|error| error.to_string())
}

fn decode_commit(bytes: &[u8]) -> Result<BranchCommit, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

fn decode(bytes: &[u8]) -> Result<Branch, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

impl BranchRegistry {
    /// Open (or create) `branch_registry.redb` under `persist_dir`.
    pub fn open(persist_dir: &str) -> Result<Self, String> {
        let path = Path::new(persist_dir).join(BRANCH_REGISTRY_FILE);
        let durable = Durable::open(
            &path,
            BRANCH_REGISTRY_PHYSICAL_STORE,
            BRANCH_REGISTRY_SCOPE_RESOURCE,
            BRANCH_REGISTRY_SCOPE_INCARNATION,
            medulla_kernel::store_authority::process_authority(),
        )?;
        Ok(Self { durable })
    }

    /// The registry row of `branch`, if one exists.
    pub fn get(&self, branch: &BranchRef) -> Result<Option<Branch>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::BRANCH_REGISTRY)?;
        let row = table
            .get(branch.registry_key().as_str())
            .map_err(|error| error.to_string())?;
        row.map(|value| decode(value.value())).transpose()
    }

    /// Write one new registry row. Refuses `BRANCH_EXISTS`, writing nothing,
    /// when the name already has a row (deleted or not).
    pub fn create(&self, branch: &Branch) -> Result<(), BranchCreateError> {
        let key = BranchRef {
            tenant: branch.tenant.clone(),
            graph: branch.graph.clone(),
            name: branch.name.clone(),
        }
        .registry_key();
        let bytes = encode(branch)?;
        let mut exists = false;
        let outcome = self.durable.maintain("branch_create", |owner| {
            let mut table = owner.open_table(medulla_storage::BRANCH_REGISTRY)?;
            if table
                .get(key.as_str())
                .map_err(|error| error.to_string())?
                .is_some()
            {
                exists = true;
                return Err(ServerErrorCode::BranchExists.as_str().to_string());
            }
            table
                .insert(key.as_str(), bytes.as_slice())
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        match outcome {
            Ok(()) => Ok(()),
            Err(_) if exists => Err(BranchCreateError::Refused(ServerErrorCode::BranchExists)),
            Err(error) => Err(BranchCreateError::Store(error)),
        }
    }
}

/// Row key of one commit: `commit:` cannot collide with a registry key, which
/// starts with a decimal length.
fn commit_row_key(branch: &BranchRef, commit_id: &str) -> String {
    format!("commit:{}:{commit_id}", branch.registry_key())
}

/// Row key of the batch-to-commit index that makes a replay return the
/// original commit.
fn batch_row_key(branch: &BranchRef, batch_id: &str) -> String {
    format!("batch:{}:{batch_id}", branch.registry_key())
}

/// The facts of one branch-scoped mutation that become a commit.
#[derive(Debug, Clone)]
pub struct BranchCommitInput {
    /// The client batch id; a replay of it returns the original commit.
    pub batch_id: String,
    /// The engine version that produced the commit.
    pub engine_version: u64,
    /// Digest of the manifest the commit applies.
    pub manifest_digest: String,
    /// The principal that made the change.
    pub actor: String,
    /// Commit time, epoch milliseconds.
    pub created_at_ms: u64,
    /// The record the commit carries (a branch write's mutation, R006), if any.
    pub payload: Option<Vec<u8>>,
}

/// Row key of the payload a commit carries (a source changeset,
/// MEDULLA-SCM-R002): `payload:` cannot collide with `commit:` or `batch:`.
fn payload_row_key(branch: &BranchRef, commit_id: &str) -> String {
    format!("payload:{}:{commit_id}", branch.registry_key())
}

/// What a commit builder returns: the commit facts plus the payload that
/// commits atomically with the head advance.
#[derive(Debug, Clone)]
pub struct CommitPlan {
    /// The engine version that produced the commit.
    pub engine_version: u64,
    /// Digest of the manifest the commit applies.
    pub manifest_digest: String,
    /// The principal that made the change.
    pub actor: String,
    /// Commit time, epoch milliseconds.
    pub created_at_ms: u64,
    /// The payload committed atomically with the head advance, if any.
    pub payload: Option<Vec<u8>>,
}

/// Reads the payload of an earlier commit inside the append transaction.
pub type PayloadReader<'a> = &'a dyn Fn(&str) -> Result<Option<Vec<u8>>, String>;

/// Why an append wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendError {
    /// The branch refused or the store failed.
    Branch(BranchCreateError),
    /// The commit builder refused; the text is the ready refusal message.
    Rejected(String),
}

/// The commit a branch write produced and whether it already existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendedCommit {
    /// The commit the write produced.
    pub commit: BranchCommit,
    /// Whether the commit already existed (an idempotent replay).
    pub replayed: bool,
}

fn branch_refusal(row: &Branch, caller_tenant: &str) -> Option<BranchMutationRefusal> {
    if row.tenant != caller_tenant {
        Some(BranchMutationRefusal::BranchTenantDenied)
    } else if row.deleted {
        Some(BranchMutationRefusal::BranchDeleted)
    } else {
        None
    }
}

impl BranchRegistry {
    /// The live branch of `branch` for `caller_tenant`, or the typed refusal
    /// that is decided before any graph change (MEDULLA-BRANCH-MERGE-R002).
    pub fn resolve_live(
        &self,
        branch: &BranchRef,
        caller_tenant: &str,
    ) -> Result<Result<Branch, BranchMutationRefusal>, String> {
        Ok(match self.get(branch)? {
            None => Err(BranchMutationRefusal::BranchUnknown),
            Some(row) => match branch_refusal(&row, caller_tenant) {
                Some(refusal) => Err(refusal),
                None => Ok(row),
            },
        })
    }

    /// The commit a prior attempt with `batch_id` appended, if any.
    pub fn commit_for_batch(
        &self,
        branch: &BranchRef,
        batch_id: &str,
    ) -> Result<Option<BranchCommit>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::BRANCH_REGISTRY)?;
        let Some(id) = table
            .get(batch_row_key(branch, batch_id).as_str())
            .map_err(|error| error.to_string())?
            .map(|value| value.value().to_vec())
        else {
            return Ok(None);
        };
        let id = String::from_utf8(id).map_err(|error| error.to_string())?;
        table
            .get(commit_row_key(branch, &id).as_str())
            .map_err(|error| error.to_string())?
            .map(|value| decode_commit(value.value()))
            .transpose()
    }

    /// Every commit of the branch, newest first, following the single parent
    /// chain from the head to the base pointer.
    pub fn chain(&self, branch: &BranchRef) -> Result<Vec<BranchCommit>, String> {
        let Some(row) = self.get(branch)? else {
            return Ok(Vec::new());
        };
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::BRANCH_REGISTRY)?;
        let mut commits = Vec::new();
        let mut cursor = row.head_commit;
        loop {
            let Some(commit) = table
                .get(commit_row_key(branch, &cursor).as_str())
                .map_err(|error| error.to_string())?
                .map(|value| decode_commit(value.value()))
                .transpose()?
            else {
                break;
            };
            cursor = commit.parents.first().cloned().unwrap_or_default();
            commits.push(commit);
        }
        Ok(commits)
    }

    /// Append one commit with the prior head as its single parent and advance
    /// the head, in ONE registry write transaction. A batch id that already
    /// has a commit returns it unchanged with `replayed` set and writes nothing.
    /// Refusals (`BRANCH_UNKNOWN`, `BRANCH_DELETED`, `BRANCH_TENANT_DENIED`)
    /// are decided inside the same transaction and write nothing.
    pub fn append_commit(
        &self,
        branch: &BranchRef,
        caller_tenant: &str,
        input: &BranchCommitInput,
    ) -> Result<AppendedCommit, BranchCreateError> {
        let plan = CommitPlan {
            engine_version: input.engine_version,
            manifest_digest: input.manifest_digest.clone(),
            actor: input.actor.clone(),
            created_at_ms: input.created_at_ms,
            payload: input.payload.clone(),
        };
        match self.append_with(branch, caller_tenant, &input.batch_id, |_, _| Ok(plan)) {
            Ok(done) => Ok(done),
            Err(AppendError::Branch(error)) => Err(error),
            Err(AppendError::Rejected(text)) => Err(BranchCreateError::Store(text)),
        }
    }

    /// The payload of one commit (MEDULLA-SCM-R002), if it carries one.
    pub fn commit_payload(
        &self,
        branch: &BranchRef,
        commit_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::BRANCH_REGISTRY)?;
        let found = table
            .get(payload_row_key(branch, commit_id).as_str())
            .map_err(|error| error.to_string())?
            .map(|value| value.value().to_vec());
        Ok(found)
    }

    /// Append one commit whose facts and payload come from `build`, which sees
    /// the current head and a reader of earlier payloads INSIDE the write
    /// transaction, so the head it extends cannot move before the append. The
    /// commit row, the payload row, the batch index row and the head advance
    /// are one registry transaction. A builder refusal writes nothing.
    pub fn append_with<F>(
        &self,
        branch: &BranchRef,
        caller_tenant: &str,
        batch_id: &str,
        build: F,
    ) -> Result<AppendedCommit, AppendError>
    where
        F: FnOnce(&str, PayloadReader<'_>) -> Result<CommitPlan, String>,
    {
        let key = branch.registry_key();
        let mut refused: Option<BranchMutationRefusal> = None;
        let mut rejected: Option<String> = None;
        let mut appended: Option<AppendedCommit> = None;
        let mut build = Some(build);
        let outcome = self.durable.maintain("branch_commit", |owner| {
            let mut table = owner.open_table(medulla_storage::BRANCH_REGISTRY)?;
            let existing = table
                .get(key.as_str())
                .map_err(|error| error.to_string())?
                .map(|value| decode(value.value()))
                .transpose()?;
            let Some(mut row) = existing else {
                refused = Some(BranchMutationRefusal::BranchUnknown);
                return Err("BRANCH_UNKNOWN".to_string());
            };
            if let Some(refusal) = branch_refusal(&row, caller_tenant) {
                refused = Some(refusal.clone());
                return Err(refusal.code().as_str().to_string());
            }
            let batch_key = batch_row_key(branch, batch_id);
            let prior = table
                .get(batch_key.as_str())
                .map_err(|error| error.to_string())?
                .map(|value| value.value().to_vec());
            if let Some(id) = prior {
                let id = String::from_utf8(id).map_err(|error| error.to_string())?;
                let bytes = table
                    .get(commit_row_key(branch, &id).as_str())
                    .map_err(|error| error.to_string())?
                    .map(|value| value.value().to_vec())
                    .ok_or_else(|| "branch batch index names a missing commit".to_string())?;
                let commit = decode_commit(&bytes)?;
                appended = Some(AppendedCommit {
                    commit,
                    replayed: true,
                });
                return Ok(());
            }
            let Some(build) = build.take() else {
                return Err("branch commit builder already consumed".to_string());
            };
            let plan = {
                let reader = |commit_id: &str| -> Result<Option<Vec<u8>>, String> {
                    table
                        .get(payload_row_key(branch, commit_id).as_str())
                        .map_err(|error| error.to_string())
                        .map(|found| found.map(|value| value.value().to_vec()))
                };
                match build(&row.head_commit, &reader) {
                    Ok(plan) => plan,
                    Err(text) => {
                        rejected = Some(text.clone());
                        return Err(text);
                    }
                }
            };
            let commit = BranchCommit::single_parent_commit(
                branch_commit_id(branch, &row.head_commit, batch_id, &plan.manifest_digest),
                branch.name.clone(),
                row.head_commit.clone(),
                plan.engine_version,
                plan.manifest_digest.clone(),
                plan.actor.clone(),
                plan.created_at_ms,
            );
            let commit_bytes =
                rmp_serde::to_vec_named(&commit).map_err(|error| error.to_string())?;
            table
                .insert(
                    commit_row_key(branch, &commit.id).as_str(),
                    commit_bytes.as_slice(),
                )
                .map_err(|error| error.to_string())?;
            if let Some(payload) = &plan.payload {
                table
                    .insert(
                        payload_row_key(branch, &commit.id).as_str(),
                        payload.as_slice(),
                    )
                    .map_err(|error| error.to_string())?;
            }
            table
                .insert(batch_key.as_str(), commit.id.as_bytes())
                .map_err(|error| error.to_string())?;
            row.head_commit = commit.id.clone();
            let row_bytes = encode(&row)?;
            table
                .insert(key.as_str(), row_bytes.as_slice())
                .map_err(|error| error.to_string())?;
            appended = Some(AppendedCommit {
                commit,
                replayed: false,
            });
            Ok(())
        });
        match (outcome, refused, rejected, appended) {
            (Ok(()), _, _, Some(done)) => Ok(done),
            (_, Some(refusal), _, _) => Err(AppendError::Branch(BranchCreateError::Refused(
                refusal.code(),
            ))),
            (_, None, Some(text), _) => Err(AppendError::Rejected(text)),
            (Ok(()), None, None, None) => Err(AppendError::Branch(BranchCreateError::Store(
                "branch commit was not applied".to_string(),
            ))),
            (Err(error), None, None, _) => {
                Err(AppendError::Branch(BranchCreateError::Store(error)))
            }
        }
    }
}

/// The registry is a bundled durable store: a backup copies the owner file
/// through its recovery seam, so a restore comes up with every branch, the
/// way the Agent Library owner file is carried.
impl medulla_server_api::api::persistence::BundledStoreSource for BranchRegistry {
    fn file_name(&self) -> &'static str {
        BRANCH_REGISTRY_FILE
    }

    fn copy_into(&self, destination: &Path) -> Result<u64, String> {
        let rows = {
            let read = self.durable.read()?;
            read.open_owner_table(medulla_storage::BRANCH_REGISTRY)?
                .len()
                .map_err(|error| error.to_string())?
        };
        let counts = medulla_storage::backup_recovery_store(self.durable.kernel(), destination)?;
        Ok(rows.saturating_add(counts.batches))
    }

    fn is_durable(&self) -> bool {
        true
    }
}

/// Open (once) the shared branch registry handle `ServerState` retains, under
/// the configured persist directory. Every caller (the served `CreateBranch`,
/// the backup) shares this one handle: redb's per-file lock admits one opener.
pub fn shared_branch_registry(
    state: &medulla_server_api::state::ServerState,
) -> Result<Arc<BranchRegistry>, String> {
    let mut guard = state.branch_registry.lock();
    if let Some(registry) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<BranchRegistry>().ok())
    {
        return Ok(registry);
    }
    let persist_dir = state
        .persist_dir
        .as_deref()
        .ok_or_else(|| "branch registry requires a configured persist directory".to_string())?;
    let registry = Arc::new(BranchRegistry::open(persist_dir)?);
    *guard = Some(Arc::clone(&registry) as medulla_server_api::state::ErasedHandle);
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Branch {
        Branch {
            tenant: "t".into(),
            graph: "g".into(),
            name: name.into(),
            base_version: 3,
            head_commit: "v3".into(),
            created_by: "agent".into(),
            created_at: 1,
            deleted: false,
        }
    }

    fn input(batch: &str, version: u64) -> BranchCommitInput {
        BranchCommitInput {
            batch_id: batch.into(),
            engine_version: version,
            manifest_digest: format!("m-{batch}"),
            actor: "agent".into(),
            created_at_ms: 5,
            payload: Some(batch.as_bytes().to_vec()),
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn commits_chain_by_one_parent_and_replay_returns_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let registry = BranchRegistry::open(dir.path().to_str().unwrap()).unwrap();
        let reference = BranchRef {
            tenant: "t".into(),
            graph: "g".into(),
            name: "f1".into(),
        };
        registry.create(&branch("f1")).unwrap();
        let first = registry
            .append_commit(&reference, "t", &input("b1", 1))
            .unwrap();
        assert!(!first.replayed);
        assert_eq!(first.commit.parents, vec!["v3".to_string()]);
        let second = registry
            .append_commit(&reference, "t", &input("b2", 2))
            .unwrap();
        assert_eq!(second.commit.parents, vec![first.commit.id.clone()]);
        let replay = registry
            .append_commit(&reference, "t", &input("b1", 1))
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.commit, first.commit);
        assert_eq!(
            registry.get(&reference).unwrap().unwrap().head_commit,
            second.commit.id
        );
        let chain = registry.chain(&reference).unwrap();
        assert_eq!(
            chain.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            vec![second.commit.id, first.commit.id]
        );
        assert_eq!(
            registry.commit_for_batch(&reference, "b1").unwrap(),
            Some(replay.commit)
        );
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn unknown_deleted_and_foreign_tenant_branches_refuse_and_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let registry = BranchRegistry::open(dir.path().to_str().unwrap()).unwrap();
        let reference = |name: &str| BranchRef {
            tenant: "t".into(),
            graph: "g".into(),
            name: name.into(),
        };
        assert_eq!(
            registry.append_commit(&reference("none"), "t", &input("b", 1)),
            Err(BranchCreateError::Refused(ServerErrorCode::BranchUnknown))
        );
        registry.create(&branch("live")).unwrap();
        assert_eq!(
            registry.append_commit(&reference("live"), "other", &input("b", 1)),
            Err(BranchCreateError::Refused(
                ServerErrorCode::BranchTenantDenied
            ))
        );
        let mut gone = branch("gone");
        gone.deleted = true;
        registry.create(&gone).unwrap();
        assert_eq!(
            registry.append_commit(&reference("gone"), "t", &input("b", 1)),
            Err(BranchCreateError::Refused(ServerErrorCode::BranchDeleted))
        );
        assert!(registry.chain(&reference("live")).unwrap().is_empty());
        assert_eq!(
            registry
                .get(&reference("live"))
                .unwrap()
                .unwrap()
                .head_commit,
            "v3"
        );
        assert_eq!(
            registry.resolve_live(&reference("gone"), "t").unwrap(),
            Err(BranchMutationRefusal::BranchDeleted)
        );
    }

    // spec: MEDULLA-BRANCH-MERGE-R001
    #[test]
    fn create_reads_back_and_refuses_a_duplicate_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        let reference = BranchRef {
            tenant: "t".into(),
            graph: "g".into(),
            name: "f1".into(),
        };
        {
            let registry = BranchRegistry::open(persist).unwrap();
            assert_eq!(registry.get(&reference).unwrap(), None);
            registry.create(&branch("f1")).unwrap();
        }
        let registry = BranchRegistry::open(persist).unwrap();
        assert_eq!(registry.get(&reference).unwrap(), Some(branch("f1")));
        let mut changed = branch("f1");
        changed.base_version = 9;
        assert_eq!(
            registry.create(&changed),
            Err(BranchCreateError::Refused(ServerErrorCode::BranchExists))
        );
        assert_eq!(registry.get(&reference).unwrap(), Some(branch("f1")));
    }
}
