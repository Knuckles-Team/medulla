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

use medulla_types::branch::{Branch, BranchRef, StoredBranchCommit};
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

fn commit_key(branch: &BranchRef, sequence: u64) -> String {
    format!("{}#{sequence:020}", branch.registry_key())
}

fn encode_commit(stored: &StoredBranchCommit) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(stored).map_err(|error| error.to_string())
}

fn decode_commit(bytes: &[u8]) -> Result<StoredBranchCommit, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

/// MEDULLA-BRANCH-MERGE-R002: the branch's single, append-only commit chain.
impl BranchRegistry {
    /// Every commit of `branch`, in commit order.
    pub fn commits(&self, branch: &BranchRef) -> Result<Vec<StoredBranchCommit>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::BRANCH_COMMITS)?;
        let (start, end) = (
            format!("{}#", branch.registry_key()),
            format!("{}$", branch.registry_key()),
        );
        let rows = table
            .range(start.as_str()..end.as_str())
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (_, value) = row.map_err(|error| error.to_string())?;
            decode_commit(value.value())
        })
        .collect()
    }

    /// Append `stored` as commit number `sequence` and move the registry row's
    /// head to it, in one admitted write. The caller holds the branch lane, so
    /// `sequence` is the chain length it read.
    pub fn append_commit(
        &self,
        branch: &BranchRef,
        sequence: u64,
        stored: &StoredBranchCommit,
    ) -> Result<(), String> {
        let (row_key, log_key) = (branch.registry_key(), commit_key(branch, sequence));
        let bytes = encode_commit(stored)?;
        let head = stored.commit.id.clone();
        self.durable.maintain("branch_commit", |owner| {
            let mut rows = owner.open_table(medulla_storage::BRANCH_REGISTRY)?;
            let mut row = rows
                .get(row_key.as_str())
                .map_err(|error| error.to_string())?
                .map(|value| decode(value.value()))
                .transpose()?
                .ok_or_else(|| ServerErrorCode::BranchUnknown.as_str().to_string())?;
            row.head_commit = head;
            let row = encode(&row)?;
            rows.insert(row_key.as_str(), row.as_slice())
                .map_err(|error| error.to_string())?;
            let mut log = owner.open_table(medulla_storage::BRANCH_COMMITS)?;
            log.insert(log_key.as_str(), bytes.as_slice())
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
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

    fn stored(id: &str, key: &str) -> StoredBranchCommit {
        StoredBranchCommit {
            commit: medulla_types::branch::BranchCommit {
                id: id.into(),
                branch: "f1".into(),
                parents: vec!["v3".into()],
                engine_version: 4,
                manifest_digest: "d".into(),
                actor: "agent".into(),
                created_at: 2,
            },
            idempotency_key: key.into(),
            write_set: vec![1],
            result: vec![2],
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn commits_append_in_order_move_the_head_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        let reference = BranchRef {
            tenant: "t".into(),
            graph: "g".into(),
            name: "f1".into(),
        };
        let sibling = BranchRef {
            name: "f10".into(),
            ..reference.clone()
        };
        {
            let registry = BranchRegistry::open(persist).unwrap();
            registry.create(&branch("f1")).unwrap();
            registry.create(&branch("f10")).unwrap();
            registry
                .append_commit(&reference, 0, &stored("c0", "k0"))
                .unwrap();
            registry
                .append_commit(&reference, 1, &stored("c1", "k1"))
                .unwrap();
            assert!(registry
                .append_commit(
                    &BranchRef {
                        name: "nope".into(),
                        ..reference.clone()
                    },
                    0,
                    &stored("x", "k")
                )
                .is_err());
        }
        let registry = BranchRegistry::open(persist).unwrap();
        let ids: Vec<String> = registry
            .commits(&reference)
            .unwrap()
            .into_iter()
            .map(|stored| stored.commit.id)
            .collect();
        assert_eq!(ids, vec!["c0".to_string(), "c1".to_string()]);
        assert!(registry.commits(&sibling).unwrap().is_empty());
        assert_eq!(registry.get(&reference).unwrap().unwrap().head_commit, "c1");
    }
}
