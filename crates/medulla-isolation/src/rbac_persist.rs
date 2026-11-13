//! Durable RBAC / identity persistence (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence, feature `security`).
//!
//! The RBAC evaluator (CONCEPT:MEDULLA-KG.compute.feature, [`crate::rbac::RbacPolicy`]) and the
//! registered [`AgentIdentity`] set both live in-memory in the
//! [`IsolationLayer`](crate::isolation::IsolationLayer). CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence makes that
//! state **durable**: it is written through to a redb table on every
//! `RbacAdmin`/`register_identity` mutation and reloaded at boot, so roles, grants
//! and identities survive a process restart.
//!
//! Design (mirrors the redb-backed cold tier, CONCEPT:MEDULLA-KG.coordination.distributed-cache-coherence):
//!   * ONE redb table `rbac` in `{persist_dir}/rbac.redb` (a separate file, like
//!     the blob CAS / cold tier), three well-known keys:
//!       - `policy`     → serde_json bytes of the whole [`RbacPolicy`];
//!       - `identities` → serde_json bytes of a `BTreeMap<agent_id, AgentIdentity>`;
//!       - `bootstrap`  → the one-time identity-bootstrap state.
//!   * The identity map is a `BTreeMap` so the persisted bytes are **deterministic**
//!     (stable key order); a save→reopen always restores the identical logical state.
//!   * A brand-new store is atomically bootstrapped with the current empty,
//!     default-deny policy image plus an explicit empty identity map. A partial or
//!     absent image after bootstrap is corruption and fails closed at boot.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use medulla_storage::{
    backup_strict_recovery_store, OwnedStoreHandle, PhysicalStoreIdentity, RbacOwner,
    ScopeGrantVerifier, ScopedRead, StorageKernel,
};
use medulla_transaction::MutationKernel;
use redb::TableDefinition;
use sha2::{Digest, Sha256};

use crate::rbac::RbacPolicy;
use medulla_types::acl::AgentIdentity;

/// `key → serde_json bytes`. One table, three well-known keys (`policy`,
/// `identities`, `bootstrap`) written in a single durable transaction
/// (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence).
const RBAC_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("rbac");
const POLICY_KEY: &str = "policy";
const IDENTITIES_KEY: &str = "identities";
const BOOTSTRAP_KEY: &str = "bootstrap";

/// Native mutation-scope identity for this store's RBAC/security-control
/// bookkeeping: tenant `"native"`, domain `ControlPlane` (mirrored below onto
/// every `MutationOperation::domain` this store writes — the native-scope
/// validator in `medulla_protocol_types::mutation_batch::validation` requires the two to be
/// exactly equal), resource `"security-control"`.
const RBAC_SCOPE_TENANT: &str = "native";
const RBAC_SCOPE_RESOURCE: &str = "security-control";
/// Fixed logical-generation id for the RBAC/security-control scope. NOT
/// derived from the resource name above: unlike a graph, this scope is never
/// deleted and recreated with a new generation for the life of one
/// `rbac.redb` file, so every `RbacStore::open` of the same physical file
/// must bind (and re-validate against) the exact same identity or fail
/// closed (the storage kernel's idempotent scope-rebinding check).
const RBAC_SCOPE_INCARNATION: &str = "rbac-security-control:v1";

/// Operator-facing identity of the ONE physical `rbac.redb` owner file
/// (`medulla_storage::PhysicalStoreIdentity`). Deliberately independent of the
/// logical serving scope above: it names the physical authority boundary the
/// storage kernel stamps into the owner manifest, so a file created for some
/// other owner can never be opened as this one.
const RBAC_PHYSICAL_STORE: &str = "eg-core:rbac-security-control";

/// Build the fixed [`medulla_types::MutationScopeIdentity`] for this store's
/// native RBAC/security-control mutation scope (see the `RBAC_SCOPE_*`
/// constants above). A plain function rather than a `once_cell`/`const`:
/// `MutationScopeIdentity::native` computes a SHA-256 identity digest, which
/// is not `const`-evaluable, and every constructor here is fallible by
/// construction (`ScopeTenantId`/`LogicalName`/`IncarnationId` validate their
/// input), so failures are propagated rather than `.unwrap()`/`.expect()`'d
/// away even though the fixed literals above are known-valid by inspection.
fn native_security_control_identity(
) -> Result<medulla_mutation_types::mutation_scope::MutationScopeIdentity, RbacPersistError> {
    medulla_mutation_types::mutation_scope::MutationScopeIdentity::fixed_native(
        RBAC_SCOPE_TENANT,
        medulla_mutation_types::mutation_scope::DurabilityDomain::ControlPlane,
        RBAC_SCOPE_RESOURCE,
        RBAC_SCOPE_INCARNATION,
    )
    .map_err(RbacPersistError::Redb)
}

/// Durable lifecycle for the only request admitted before an identity exists.
/// `Consumed` is never inferred from an empty identity map: removing every
/// identity therefore fails closed instead of silently reopening bootstrap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityBootstrapState {
    #[default]
    Pending,
    Consumed,
}

/// Errors from the durable RBAC store (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence). redb's several fallible
/// surfaces are flattened to a message string (matching the cold-tier convention);
/// io + serde carry their native errors so callers can inspect them.
#[derive(Debug)]
pub enum RbacPersistError {
    /// Creating the persist dir / opening the redb file failed.
    Io(std::io::Error),
    /// (De)serializing the policy or the identity map failed.
    Serde(serde_json::Error),
    /// A redb transaction/table/storage/commit operation failed.
    Redb(String),
    /// One or both mandatory current state records are absent.
    IncompleteState(&'static str),
    /// The image was derived from a revision that is no longer current.
    StaleRevision { expected: u64, actual: u64 },
    /// `RBAC_RESERVED`: a durable reservation holds the authorization image.
    Reserved { txn_id: String },
    /// `RBAC_REVISION_STALE`: a reservation named a revision that is no longer current.
    ReservationStale { expected: u64, actual: u64 },
    /// `RBAC_RESERVATION_UNKNOWN`: the transaction holds no reservation.
    ReservationUnknown(String),
    /// The reservation or grant lifecycle machine refused the decision.
    Machine(medulla_types::transition_refused::TransitionRefused),
    /// `RBAC_LAYOUT_OUTDATED`: the file is a predecessor layout; an operator upgrade is required.
    LayoutOutdated(String),
    /// `RBAC_UPGRADE_SOURCE_CHANGED`: the file changed after its inspection.
    UpgradeSourceChanged(String),
    /// `RBAC_GRANT_UNKNOWN`: a revoke named a grant that does not exist.
    GrantUnknown(String),
}

impl fmt::Display for RbacPersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RbacPersistError::Io(e) => write!(f, "rbac persist io error: {e}"),
            RbacPersistError::Serde(e) => write!(f, "rbac persist serde error: {e}"),
            RbacPersistError::Redb(e) => write!(f, "rbac persist redb error: {e}"),
            RbacPersistError::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "RBAC_STALE_REVISION: expected {expected}, current {actual}"
                )
            }
            RbacPersistError::Reserved { txn_id } => {
                write!(
                    f,
                    "RBAC_RESERVED: the authorization image is reserved by '{txn_id}'"
                )
            }
            RbacPersistError::ReservationStale { expected, actual } => {
                write!(
                    f,
                    "RBAC_REVISION_STALE: expected {expected}, current {actual}"
                )
            }
            RbacPersistError::ReservationUnknown(txn_id) => {
                write!(
                    f,
                    "RBAC_RESERVATION_UNKNOWN: '{txn_id}' holds no reservation"
                )
            }
            RbacPersistError::Machine(refused) => write!(f, "{refused}"),
            RbacPersistError::LayoutOutdated(detail) => {
                write!(f, "RBAC_LAYOUT_OUTDATED: {detail}")
            }
            RbacPersistError::UpgradeSourceChanged(detail) => {
                write!(f, "RBAC_UPGRADE_SOURCE_CHANGED: {detail}")
            }
            RbacPersistError::GrantUnknown(id) => {
                write!(f, "RBAC_GRANT_UNKNOWN: '{id}' is not a profile-use grant")
            }
            RbacPersistError::IncompleteState(message) => {
                write!(f, "rbac persist current-state error: {message}")
            }
        }
    }
}

impl std::error::Error for RbacPersistError {}

impl From<std::io::Error> for RbacPersistError {
    fn from(e: std::io::Error) -> Self {
        RbacPersistError::Io(e)
    }
}

impl From<serde_json::Error> for RbacPersistError {
    fn from(e: serde_json::Error) -> Self {
        RbacPersistError::Serde(e)
    }
}

/// One atomic authorization image used by every policy decision lease.
///
/// The revision and both canonical digests are captured from the same store
/// snapshot as the policy and identity map. Consumers must never reconstruct
/// this object from separate `load` and `current_version` calls.
pub struct RbacAuthoritySnapshot {
    revision: u64,
    policy_digest: String,
    identity_digest: String,
    policy: RbacPolicy,
    identities: BTreeMap<String, AgentIdentity>,
    bootstrap: IdentityBootstrapState,
}

impl RbacAuthoritySnapshot {
    fn from_parts(
        revision: u64,
        policy: RbacPolicy,
        identities: BTreeMap<String, AgentIdentity>,
        bootstrap: IdentityBootstrapState,
    ) -> Result<Self, RbacPersistError> {
        Ok(Self {
            revision,
            policy_digest: authority_policy_digest(&policy)?,
            identity_digest: authority_identity_digest(&identities)?,
            policy,
            identities,
            bootstrap,
        })
    }

    pub fn bootstrap(&self) -> IdentityBootstrapState {
        self.bootstrap
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    pub fn identity_digest(&self) -> &str {
        &self.identity_digest
    }

    pub fn policy(&self) -> &RbacPolicy {
        &self.policy
    }

    pub fn identities(&self) -> &BTreeMap<String, AgentIdentity> {
        &self.identities
    }
}

pub(crate) fn authority_policy_digest(policy: &RbacPolicy) -> Result<String, RbacPersistError> {
    #[derive(serde::Serialize)]
    struct CanonicalPolicy<'a> {
        roles: BTreeMap<&'a str, &'a medulla_types::acl::Role>,
        grants: &'a [medulla_types::acl::Grant],
        // EH-404: an elevation transition (above all a revocation) moves the
        // digest, so a policy decision lease minted before it fails closed.
        // Omitted while empty, so an image without elevations keeps its digest.
        #[serde(skip_serializing_if = "no_elevations")]
        elevations: &'a medulla_identity_types::rbac_elevation::ElevationLedger,
    }
    fn no_elevations(ledger: &&medulla_identity_types::rbac_elevation::ElevationLedger) -> bool {
        ledger.is_empty()
    }
    let canonical = CanonicalPolicy {
        roles: policy
            .roles()
            .map(|role| (role.name.as_str(), role))
            .collect(),
        grants: policy.grants(),
        elevations: policy.elevations(),
    };
    let mut hasher = Sha256::new();
    hasher.update(b"eg/rbac-policy-snapshot/v1\0");
    hasher.update(serde_json::to_vec(&canonical)?);
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn authority_identity_digest(
    identities: &BTreeMap<String, AgentIdentity>,
) -> Result<String, RbacPersistError> {
    let mut hasher = Sha256::new();
    hasher.update(b"eg/rbac-identity-snapshot/v1\0");
    hasher.update(serde_json::to_vec(identities)?);
    Ok(hex::encode(hasher.finalize()))
}

fn read_authority_image(
    read: &ScopedRead<'_, RbacOwner>,
) -> Result<
    (
        RbacPolicy,
        BTreeMap<String, AgentIdentity>,
        IdentityBootstrapState,
    ),
    RbacPersistError,
> {
    let state = read
        .open_owner_table(RBAC_TABLE)
        .map_err(RbacPersistError::Redb)?;
    let policy = state
        .get(POLICY_KEY)
        .map_err(|error| RbacPersistError::Redb(error.to_string()))?
        .ok_or(RbacPersistError::IncompleteState(
            "mandatory policy record is absent",
        ))
        .and_then(|value| serde_json::from_slice(value.value()).map_err(RbacPersistError::Serde))?;
    let identities = state
        .get(IDENTITIES_KEY)
        .map_err(|error| RbacPersistError::Redb(error.to_string()))?
        .ok_or(RbacPersistError::IncompleteState(
            "mandatory identities record is absent",
        ))
        .and_then(|value| serde_json::from_slice(value.value()).map_err(RbacPersistError::Serde))?;
    let bootstrap: IdentityBootstrapState = state
        .get(BOOTSTRAP_KEY)
        .map_err(|error| RbacPersistError::Redb(error.to_string()))?
        .ok_or(RbacPersistError::IncompleteState(
            "mandatory identity bootstrap record is absent",
        ))
        .and_then(|value| serde_json::from_slice(value.value()).map_err(RbacPersistError::Serde))?;
    Ok((policy, identities, bootstrap))
}

/// A durable, kernel-backed image of the RBAC policy + registered identities
/// (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence).
///
/// RF-RULING-004: this domain crate owns no physical authority. The
/// [`StorageKernel`] it holds is the sole owner of the physical `rbac.redb`
/// file, opened under the declared [`medulla_storage::OwnerLayout::Rbac`] whose only
/// owner table is `rbac`; every read is a kernel-issued
/// [`ScopedRead`] and every write-through is admitted, ordered and committed by
/// the [`MutationKernel`] that holds the file's single move-once mutation
/// authority. `owner` is the one authenticated, bound serving scope (see
/// `native_security_control_identity`), validated once at open.
/// The canonical digest of the whole authorization image -- the policy
/// digest and the identity digest named together -- so an identity or RBAC
/// write can record the change of the image in one audit field.
pub(crate) fn authorization_digest(
    policy: &RbacPolicy,
    identities: &BTreeMap<String, AgentIdentity>,
) -> Result<String, RbacPersistError> {
    Ok(format!(
        "{}:{}",
        authority_policy_digest(policy)?,
        authority_identity_digest(identities)?
    ))
}

pub struct RbacStore {
    kernel: StorageKernel,
    mutations: MutationKernel,
    owner: OwnedStoreHandle<RbacOwner>,
}

/// Adapter seam for durable identity/RBAC policy state.  Production uses
/// [`RbacStore`]; tests or alternate storage backends can implement the same
/// atomic snapshot contract without changing [`IsolationLayer`](crate::isolation::IsolationLayer).
pub trait RbacPolicyStore: Send + Sync {
    fn load(
        &self,
    ) -> Result<
        (
            RbacPolicy,
            BTreeMap<String, AgentIdentity>,
            IdentityBootstrapState,
        ),
        RbacPersistError,
    >;
    /// Unversioned writes cannot prove which image the caller edited.
    /// Retained for source compatibility; use `compare_and_save` instead.
    fn save(
        &self,
        _policy: &RbacPolicy,
        _identities: &BTreeMap<String, AgentIdentity>,
        _bootstrap: IdentityBootstrapState,
    ) -> Result<(), RbacPersistError> {
        Err(RbacPersistError::IncompleteState(
            "RBAC_EXPECTED_REVISION_REQUIRED",
        ))
    }

    /// Compare the caller snapshot revision under the write lock and return
    /// the revision committed with the complete replacement image. Adapters
    /// must implement this atomically; a legacy save is not a safe fallback.
    fn compare_and_save(
        &self,
        _expected_revision: u64,
        _policy: &RbacPolicy,
        _identities: &BTreeMap<String, AgentIdentity>,
        _bootstrap: IdentityBootstrapState,
    ) -> Result<u64, RbacPersistError> {
        Err(RbacPersistError::IncompleteState(
            "RBAC_CHECKED_WRITE_UNSUPPORTED",
        ))
    }

    /// Capture revision, policy, identities, and their canonical digests from
    /// one store snapshot. This is the only valid input to a policy decision
    /// lease; separate reads are intentionally insufficient.
    fn authority_snapshot(&self) -> Result<RbacAuthoritySnapshot, RbacPersistError>;

    /// Reserve the authorization image for a cross-owner admission. `None` means this
    /// adapter has no durable reservation (the in-memory store).
    fn reserve_authority(
        &self,
        _expected_revision: u64,
        _txn_id: &str,
    ) -> Result<Option<ReservationRecord>, RbacPersistError> {
        Ok(None)
    }

    /// Resolve a reservation with the durable coordinator decision.
    fn release_authority(
        &self,
        _txn_id: &str,
        _decision: Option<medulla_types::reservation_machine::ReservationEvent>,
    ) -> Result<(), RbacPersistError> {
        Ok(())
    }

    /// Every durable profile-use grant (MEDULLA-DECISION-ENGINE-R036.106). The in-memory store
    /// holds none.
    fn profile_use_grants(&self) -> Result<Vec<ProfileUseGrant>, RbacPersistError> {
        Ok(Vec::new())
    }

    /// Copy this store's durable image verbatim into a FRESH bundle file at
    /// `destination`, returning the number of rows copied.
    ///
    /// `None` ⇒ this adapter has no on-disk file to bundle (the in-memory store).
    /// An online backup that omits this file restores an engine with NO RBAC or
    /// identity state at all — every sign-in fails closed — so the durable
    /// implementation MUST override this.
    fn backup_into(&self, _destination: &Path) -> Option<Result<u64, String>> {
        None
    }

    /// `true` when this adapter owns an on-disk file that a backup must capture.
    fn has_durable_file(&self) -> bool {
        false
    }

    /// Existing mutation revision, useful for diagnostics. Never combine a
    /// separate version read with an image to construct a write precondition;
    /// use `authority_snapshot` for the complete atomic preimage instead.
    fn current_version(&self) -> Result<u64, RbacPersistError> {
        Err(RbacPersistError::IncompleteState(
            "this policy store does not implement a durable policy version counter",
        ))
    }
}

mod durable_write;
mod memory_store;
mod profile_grant;
mod reservation;
pub use profile_grant::{
    inspect_rbac_layout, upgrade_rbac_layout, InspectedRbacLayout, ProfileUseGrant,
    RbacLayoutUpgradeReport, RBAC_GRANT_UNKNOWN, RBAC_LAYOUT_OUTDATED, RBAC_UPGRADE_SOURCE_CHANGED,
};
pub use reservation::{
    ReservationRecord, RBAC_RESERVATION_UNKNOWN, RBAC_RESERVED, RBAC_REVISION_STALE,
};
#[cfg(test)]
mod revision_tests;

pub use memory_store::MemoryRbacStore;

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::test_scope_grant::{TestScopeVerifier, TEST_PRINCIPAL, TEST_PROOF};
    use medulla_storage::OwnerLayout;

    /// Open the durable RBAC store the way the composition root would.
    pub(crate) fn open_test_store(dir: impl AsRef<Path>) -> Result<RbacStore, RbacPersistError> {
        RbacStore::open(
            dir,
            &TestScopeVerifier {
                layout: OwnerLayout::Rbac,
            },
            TEST_PRINCIPAL,
            TEST_PROOF,
        )
    }
}

impl fmt::Debug for RbacStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RbacStore").finish_non_exhaustive()
    }
}

impl RbacStore {
    /// Open (or create) `{dir}/rbac.redb` through the storage kernel
    /// (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence). The dir is
    /// created if absent.
    ///
    /// The kernel creates the file under [`medulla_storage::OwnerLayout::Rbac`],
    /// which declares `rbac` as the layout's one owner table, so the
    /// hand-written bootstrap closure the retired raw constructor needed is
    /// gone: the declared census is opened atomically at create time and
    /// re-validated on every later open.
    ///
    /// `verifier` is the composition root's proof authority: only it may decide
    /// that `principal` is entitled to serve this store's fixed
    /// `security-control` scope. The domain crate supplies the scope identity
    /// and the layout; it never interprets the proof bytes.
    pub fn open<P: AsRef<Path>>(
        dir: P,
        verifier: &dyn ScopeGrantVerifier,
        principal: &str,
        proof: &[u8],
    ) -> Result<Self, RbacPersistError> {
        std::fs::create_dir_all(dir.as_ref())?;
        let path = dir.as_ref().join("rbac.redb");
        let identity = native_security_control_identity()?;
        let physical =
            PhysicalStoreIdentity::new(RBAC_PHYSICAL_STORE).map_err(RbacPersistError::Redb)?;
        let kernel = if path.exists() {
            StorageKernel::open_owner::<RbacOwner>(&path, physical, None)
        } else {
            StorageKernel::create_owner::<RbacOwner>(&path, physical, None)
        }
        .map_err(|error| {
            if error.starts_with("RBAC_FORMAT_UPGRADE_REQUIRED") {
                RbacPersistError::LayoutOutdated(error)
            } else {
                RbacPersistError::Redb(error)
            }
        })?;
        let (kernel, authority) = kernel
            .into_read_and_mutation_authority()
            .map_err(RbacPersistError::Redb)?;
        let mutations = MutationKernel::new(authority);
        let grant = kernel
            .authenticate_scope::<RbacOwner>(verifier, identity, principal.to_string(), proof)
            .map_err(RbacPersistError::Redb)?;
        // `initial_version: 0` -- the SAME "a brand new store starts at version
        // 0" semantics `save`/`current_version` rely on. Binding the identical
        // grant again on a later open is idempotent; any generation, tenant,
        // domain, resource or initial-version mismatch fails closed.
        let owner = kernel
            .bind_serving_scope(grant, 0)
            .map_err(RbacPersistError::Redb)?;
        mutations
            .bootstrap_ledger(&owner)
            .map_err(RbacPersistError::Redb)?;
        let store = Self {
            kernel,
            mutations,
            owner,
        };
        store.bootstrap_current_state()?;
        Ok(store)
    }

    /// One kernel-issued scoped read over this store's bound serving scope.
    pub(super) fn scoped_read(&self) -> Result<ScopedRead<'_, RbacOwner>, RbacPersistError> {
        self.kernel
            .read_scope(&self.owner)
            .map_err(RbacPersistError::Redb)
    }

    /// Copy the durable RBAC/identity image into a FRESH file at `destination`
    /// (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence), returning the
    /// number of rows copied. Refuses to overwrite an existing file.
    ///
    /// Delegated whole to [`backup_strict_recovery_store`], which owns the
    /// complete table census: it copies the ledger rows, re-stamps
    /// `SCOPE_BINDINGS` for the destination's own incarnation, copies every
    /// declared owner table of the layout (here `rbac`), and reopens the
    /// destination to prove the per-table fingerprints match the source.
    /// Hand-listing tables here is what produced BUG-PE-054, and appending
    /// `rbac` afterwards through a private `Database::create` made this
    /// crate a second physical authority -- both are deleted.
    pub fn backup_into(&self, destination: &Path) -> Result<u64, String> {
        if destination.exists() {
            return Err("bundled store file already exists (refusing to overwrite)".to_string());
        }
        let identity = PhysicalStoreIdentity::new(RBAC_PHYSICAL_STORE)?;
        let evidence = backup_strict_recovery_store(&self.kernel, destination, identity)?;
        Ok(evidence.ledger_rows.saturating_add(evidence.owner_rows))
    }

    /// Atomically create the explicit current bootstrap image for a brand-new store.
    /// A partial image is never repaired because that could silently discard part
    /// of an authorization state transition.
    fn bootstrap_current_state(&self) -> Result<(), RbacPersistError> {
        let read = self.scoped_read()?;
        let table = read
            .open_owner_table(RBAC_TABLE)
            .map_err(RbacPersistError::Redb)?;
        let policy_present = table
            .get(POLICY_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
            .is_some();
        let identities_present = table
            .get(IDENTITIES_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
            .is_some();
        let bootstrap_present = table
            .get(BOOTSTRAP_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
            .is_some();
        drop(table);
        drop(read);
        match (policy_present, identities_present, bootstrap_present) {
            (true, true, true) => Ok(()),
            (false, false, false) => durable_write::initialize_authority_state(self),
            _ => Err(RbacPersistError::IncompleteState(
                "policy, identities, and bootstrap state must all be present",
            )),
        }
    }

    /// Load the mandatory persisted policy, identities, and bootstrap lifecycle
    /// (CONCEPT:MEDULLA-KG.compute.durable-rbac-identity-persistence). Missing records are
    /// rejected; only [`RbacStore::open`] may create the explicit bootstrap image.
    pub fn load(
        &self,
    ) -> Result<
        (
            RbacPolicy,
            BTreeMap<String, AgentIdentity>,
            IdentityBootstrapState,
        ),
        RbacPersistError,
    > {
        let read = self.scoped_read()?;
        let t = read
            .open_owner_table(RBAC_TABLE)
            .map_err(RbacPersistError::Redb)?;
        let policy = match t
            .get(POLICY_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
        {
            Some(v) => serde_json::from_slice(v.value())?,
            None => {
                return Err(RbacPersistError::IncompleteState(
                    "mandatory policy record is absent",
                ))
            }
        };
        let identities = match t
            .get(IDENTITIES_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
        {
            Some(v) => serde_json::from_slice(v.value())?,
            None => {
                return Err(RbacPersistError::IncompleteState(
                    "mandatory identities record is absent",
                ))
            }
        };
        let bootstrap = match t
            .get(BOOTSTRAP_KEY)
            .map_err(|e| RbacPersistError::Redb(e.to_string()))?
        {
            Some(v) => serde_json::from_slice(v.value())?,
            None => {
                return Err(RbacPersistError::IncompleteState(
                    "mandatory identity bootstrap record is absent",
                ))
            }
        };
        Ok((policy, identities, bootstrap))
    }

    /// Read the complete authorization image and its mutation revision from
    /// one redb MVCC transaction. This prevents a lease from combining policy
    /// bytes from one commit with the revision or identities from another.
    pub fn authority_snapshot(&self) -> Result<RbacAuthoritySnapshot, RbacPersistError> {
        let read = self.scoped_read()?;
        let (policy, identities, bootstrap) = read_authority_image(&read)?;
        let revision = medulla_transaction::version(&read).map_err(RbacPersistError::Redb)?;
        RbacAuthoritySnapshot::from_parts(revision, policy, identities, bootstrap)
    }

    /// Unversioned writes fail closed. Use the snapshot revision with
    /// `compare_and_save`; reading the latest counter just before saving a
    /// previously loaded image does not establish a valid precondition.
    pub fn save(
        &self,
        policy: &RbacPolicy,
        identities: &BTreeMap<String, AgentIdentity>,
        bootstrap: IdentityBootstrapState,
    ) -> Result<(), RbacPersistError> {
        <Self as RbacPolicyStore>::save(self, policy, identities, bootstrap)
    }

    /// Replace the full image only if the caller's atomic snapshot is current.
    pub fn compare_and_save(
        &self,
        expected_revision: u64,
        policy: &RbacPolicy,
        identities: &BTreeMap<String, AgentIdentity>,
        bootstrap: IdentityBootstrapState,
    ) -> Result<u64, RbacPersistError> {
        durable_write::save_authority_state(self, expected_revision, policy, identities, bootstrap)
    }

    /// Remove one mandatory durable record through the mutation kernel, so a
    /// test can produce a known-bad partial image without opening a store.
    #[cfg(test)]
    pub(super) fn remove_mandatory_record_for_test(
        &self,
        key: &'static str,
    ) -> Result<(), RbacPersistError> {
        durable_write::remove_authority_record(self, key)
    }

    /// GRAPH-POLICY-LEASE-CONTRACT.md §2.4 (R5): the SAME mutation-ledger
    /// counter [`RbacStore::compare_and_save`] bumps atomically (inside the one
    /// admitted write transaction, above) with the policy/identity/bootstrap
    /// writes -- reused here rather than adding a second, independently-
    /// maintained persisted counter.
    pub fn current_version(&self) -> Result<u64, RbacPersistError> {
        let read = self.scoped_read()?;
        medulla_transaction::version(&read).map_err(RbacPersistError::Redb)
    }
}

impl RbacPolicyStore for RbacStore {
    fn load(
        &self,
    ) -> Result<
        (
            RbacPolicy,
            BTreeMap<String, AgentIdentity>,
            IdentityBootstrapState,
        ),
        RbacPersistError,
    > {
        RbacStore::load(self)
    }

    fn compare_and_save(
        &self,
        expected_revision: u64,
        policy: &RbacPolicy,
        identities: &BTreeMap<String, AgentIdentity>,
        bootstrap: IdentityBootstrapState,
    ) -> Result<u64, RbacPersistError> {
        RbacStore::compare_and_save(self, expected_revision, policy, identities, bootstrap)
    }

    fn authority_snapshot(&self) -> Result<RbacAuthoritySnapshot, RbacPersistError> {
        RbacStore::authority_snapshot(self)
    }

    fn reserve_authority(
        &self,
        expected_revision: u64,
        txn_id: &str,
    ) -> Result<Option<ReservationRecord>, RbacPersistError> {
        RbacStore::reserve(self, expected_revision, txn_id).map(Some)
    }

    fn release_authority(
        &self,
        txn_id: &str,
        decision: Option<medulla_types::reservation_machine::ReservationEvent>,
    ) -> Result<(), RbacPersistError> {
        RbacStore::release(self, txn_id, decision).map(|_| ())
    }

    fn profile_use_grants(&self) -> Result<Vec<ProfileUseGrant>, RbacPersistError> {
        RbacStore::profile_use_grants(self)
    }

    fn backup_into(&self, destination: &Path) -> Option<Result<u64, String>> {
        Some(RbacStore::backup_into(self, destination))
    }

    fn has_durable_file(&self) -> bool {
        true
    }

    fn current_version(&self) -> Result<u64, RbacPersistError> {
        RbacStore::current_version(self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::test_support::open_test_store;
    use super::{
        IdentityBootstrapState, MemoryRbacStore, RbacPersistError, RbacPolicyStore, IDENTITIES_KEY,
    };
    use crate::rbac::RbacPolicy;
    use medulla_types::acl::{
        AgentIdentity, AgentRole, Grant, GrantEffect, RbacAction, ResourceContext,
        ResourceSelector, Role,
    };

    /// A unique temp dir per test invocation (no external dev-dep needed).
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eg303-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn identity(id: &str, roles: Vec<String>) -> AgentIdentity {
        AgentIdentity {
            agent_id: id.to_string(),
            role: AgentRole::Agent,
            teams: vec![],
            roles,
        }
    }

    #[test]
    fn eg303_store_saves_and_reloads_policy_and_identities() {
        let dir = tmp_dir("store-rt");
        let mut policy = RbacPolicy::new();
        policy.add_role(Role::with_parents("editor", vec!["reader".into()]));
        policy.add_grant(Grant {
            role: "editor".into(),
            resource: ResourceSelector::Label("Doc".into()),
            action: RbacAction::Write,
            effect: GrantEffect::Allow,
        });
        let mut identities = BTreeMap::new();
        identities.insert("sam".to_string(), identity("sam", vec!["editor".into()]));

        {
            let store = open_test_store(&dir).unwrap();
            store
                .compare_and_save(
                    store.authority_snapshot().unwrap().revision(),
                    &policy,
                    &identities,
                    IdentityBootstrapState::Consumed,
                )
                .unwrap();
        }
        // Reopen the SAME dir — the state is durable across "process" lifetimes.
        let store = open_test_store(&dir).unwrap();
        let (loaded_policy, loaded_ids, bootstrap) = store.load().unwrap();
        assert_eq!(loaded_policy.grants().len(), 1);
        assert!(loaded_policy.is_allowed(
            &["editor"],
            &ResourceContext {
                graph: "g".into(),
                label: Some("Doc".into())
            },
            RbacAction::Write
        ));
        assert_eq!(loaded_ids.len(), 1);
        assert_eq!(loaded_ids["sam"].roles, vec!["editor".to_string()]);
        assert_eq!(bootstrap, IdentityBootstrapState::Consumed);
        let authority = store.authority_snapshot().unwrap();
        assert_eq!(authority.policy().grants().len(), 1);
        assert_eq!(
            authority.identities()["sam"].roles,
            vec!["editor".to_string()]
        );
        assert!(authority.revision() > 0);
        assert_eq!(authority.policy_digest().len(), 64);
        assert_eq!(authority.identity_digest().len(), 64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_authority_snapshot_captures_revision_and_state_under_one_lock() {
        let store = MemoryRbacStore::new();
        let mut identities = BTreeMap::new();
        identities.insert("sam".to_string(), identity("sam", vec!["reader".into()]));
        store
            .compare_and_save(
                store.authority_snapshot().unwrap().revision(),
                &RbacPolicy::new(),
                &identities,
                IdentityBootstrapState::Consumed,
            )
            .unwrap();

        let authority = store.authority_snapshot().unwrap();
        assert_eq!(authority.revision(), 1);
        assert_eq!(
            authority.identities()["sam"].roles,
            vec!["reader".to_string()]
        );
        assert_eq!(authority.policy_digest().len(), 64);
        assert_eq!(authority.identity_digest().len(), 64);
    }

    #[test]
    fn eg303_new_store_bootstraps_explicit_default_deny_state() {
        let dir = tmp_dir("absent");
        let store = open_test_store(&dir).unwrap();
        let (policy, ids, bootstrap) = store.load().unwrap();
        assert!(policy.grants().is_empty());
        assert!(ids.is_empty());
        assert_eq!(bootstrap, IdentityBootstrapState::Pending);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consumed_bootstrap_never_reopens_when_identities_are_empty() {
        let dir = tmp_dir("bootstrap-consumed");
        {
            let store = open_test_store(&dir).unwrap();
            store
                .compare_and_save(
                    store.authority_snapshot().unwrap().revision(),
                    &RbacPolicy::new(),
                    &BTreeMap::new(),
                    IdentityBootstrapState::Consumed,
                )
                .unwrap();
        }
        let store = open_test_store(&dir).unwrap();
        let (_, identities, bootstrap) = store.load().unwrap();
        assert!(identities.is_empty());
        assert_eq!(bootstrap, IdentityBootstrapState::Consumed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eg303_partial_current_state_fails_closed() {
        let dir = tmp_dir("partial");
        let store = open_test_store(&dir).unwrap();
        // A known-bad durable input, produced the ONLY way this crate can now
        // write: an admitted mutation through the mutation kernel. The store
        // must still refuse to serve, and refuse to reopen, a partial image.
        store
            .remove_mandatory_record_for_test(IDENTITIES_KEY)
            .unwrap();
        assert!(matches!(
            store.load(),
            Err(RbacPersistError::IncompleteState(_))
        ));
        drop(store);
        assert!(matches!(
            open_test_store(&dir),
            Err(RbacPersistError::IncompleteState(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eg303_identity_bytes_are_deterministic() {
        // The BTreeMap identity container gives a stable byte serialization: two
        // saves of the same logical state produce identical persisted bytes.
        let mut a = BTreeMap::new();
        a.insert("b".to_string(), identity("b", vec!["r".into()]));
        a.insert("a".to_string(), identity("a", vec![]));
        // Insert in the OTHER order — BTreeMap normalizes ordering.
        let mut b = BTreeMap::new();
        b.insert("a".to_string(), identity("a", vec![]));
        b.insert("b".to_string(), identity("b", vec!["r".into()]));
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
    }
    // spec: MEDULLA-DECISION-ENGINE-R036.103
    #[test]
    fn reservation_blocks_writes_and_survives_reopen() {
        use medulla_types::reservation_machine::{ReservationEvent, ReservationState};

        let dir = tmp_dir("reservation");
        let store = open_test_store(&dir).unwrap();
        let revision = store.authority_snapshot().unwrap().revision();
        let policy = RbacPolicy::new();
        let none = BTreeMap::new();

        // A stale revision refuses by name and writes nothing.
        let error = store.reserve(revision + 5, "txn-stale").unwrap_err();
        assert!(
            error.to_string().starts_with("RBAC_REVISION_STALE"),
            "{error}"
        );
        assert!(store.reservations().unwrap().is_empty());

        // The reservation carries the revision read and both digests of the snapshot.
        let held = store.reserve(revision, "txn-1").unwrap();
        let snapshot_digests = (
            super::authority_policy_digest(&policy).unwrap(),
            super::authority_identity_digest(&none).unwrap(),
        );
        assert_eq!(held.revision, revision);
        assert_eq!(held.state, ReservationState::Reserved);
        assert_eq!(
            (held.policy_digest.clone(), held.identity_digest.clone()),
            snapshot_digests
        );
        assert_eq!(held.premise(), format!("rbac_revision:{revision}"));
        // An exact retry returns the held record without a second write.
        assert_eq!(store.reserve(revision, "txn-1").unwrap(), held);

        // Competing saves and a competing reservation are refused.
        let current = store.authority_snapshot().unwrap().revision();
        let error = store
            .compare_and_save(current, &policy, &none, IdentityBootstrapState::Pending)
            .unwrap_err();
        assert!(error.to_string().starts_with("RBAC_RESERVED"), "{error}");
        let error = store.reserve(current, "txn-2").unwrap_err();
        assert!(error.to_string().starts_with("RBAC_RESERVED"), "{error}");
        assert_eq!(store.authority_snapshot().unwrap().revision(), current);

        // A reopen keeps the open reservation, and it still blocks.
        drop(store);
        let store = open_test_store(&dir).unwrap();
        assert_eq!(store.reservations().unwrap(), vec![held.clone()]);
        let current = store.authority_snapshot().unwrap().revision();
        assert!(store
            .compare_and_save(current, &policy, &none, IdentityBootstrapState::Pending)
            .is_err());

        // A missing decision and an unknown transaction leave it held.
        let error = store.release("txn-1", None).unwrap_err();
        assert!(error.to_string().starts_with("RBAC_RESERVED"), "{error}");
        let error = store
            .release("txn-unknown", Some(ReservationEvent::Commit))
            .unwrap_err();
        assert!(
            error.to_string().starts_with("RBAC_RESERVATION_UNKNOWN"),
            "{error}"
        );

        // The durable decision resolves it; the repeat replays; the opposite is refused.
        let resolved = store
            .release("txn-1", Some(ReservationEvent::Commit))
            .unwrap();
        assert_eq!(resolved.state, ReservationState::Committed);
        let version = store.current_version().unwrap();
        assert_eq!(
            store
                .release("txn-1", Some(ReservationEvent::Commit))
                .unwrap(),
            resolved
        );
        assert_eq!(
            store.current_version().unwrap(),
            version,
            "replay writes nothing"
        );
        let error = store
            .release("txn-1", Some(ReservationEvent::Abort))
            .unwrap_err();
        assert!(error.to_string().contains("TRANSITION_REFUSED"), "{error}");

        // Writes pass again, and an aborted reservation also releases the image.
        let current = store.authority_snapshot().unwrap().revision();
        store
            .compare_and_save(current, &policy, &none, IdentityBootstrapState::Pending)
            .unwrap();
        let current = store.authority_snapshot().unwrap().revision();
        store.reserve(current, "txn-3").unwrap();
        let aborted = store
            .release("txn-3", Some(ReservationEvent::Abort))
            .unwrap();
        assert_eq!(aborted.state, ReservationState::Aborted);
        let current = store.authority_snapshot().unwrap().revision();
        store
            .compare_and_save(current, &policy, &none, IdentityBootstrapState::Pending)
            .unwrap();
    }

    /// A private staging directory inside the crate checkout (the inspection refuses `/tmp`).
    fn private_staging(tag: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "upgrade-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    /// Every `rbac` row of the store file in `dir`, by key.
    fn rbac_rows(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
        use redb::{ReadableDatabase, ReadableTable};
        let database = redb::Database::open(dir.join("rbac.redb")).unwrap();
        let read = database.begin_read().unwrap();
        let table = read.open_table(super::RBAC_TABLE).unwrap();
        table
            .iter()
            .unwrap()
            .map(|row| {
                let (key, value) = row.unwrap();
                (key.value().to_string(), value.value().to_vec())
            })
            .collect()
    }

    /// A predecessor file at `dir/rbac.redb` carrying `rows` in its `rbac` table.
    fn predecessor_with(dir: &std::path::Path, rows: &BTreeMap<String, Vec<u8>>) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("rbac.redb");
        medulla_storage::create_predecessor_owner_file(
            &path,
            medulla_storage::PhysicalStoreIdentity::new(super::RBAC_PHYSICAL_STORE).unwrap(),
            &medulla_storage::RBAC_BEFORE_PROFILE_GRANTS,
        )
        .unwrap();
        let seeded: Vec<(&str, &[u8])> = rows
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_slice()))
            .collect();
        medulla_storage::write_fixture_rows(&path, super::RBAC_TABLE, &seeded).unwrap();
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.106
    #[test]
    fn upgrade_preserves_rows_and_creates_empty_grant_table() {
        use medulla_types::reservation_machine::ReservationEvent;

        // The authority and reservation rows of a real current store.
        let source = tmp_dir("grant-source");
        let store = open_test_store(&source).unwrap();
        let revision = store.authority_snapshot().unwrap().revision();
        store.reserve(revision, "txn-kept").unwrap();
        store
            .release("txn-kept", Some(ReservationEvent::Commit))
            .unwrap();
        let revision = store.authority_snapshot().unwrap().revision();
        store.reserve(revision, "txn-open").unwrap();
        let expected = (
            store
                .authority_snapshot()
                .unwrap()
                .policy_digest()
                .to_string(),
            store.reservations().unwrap(),
        );
        drop(store);
        let rows = rbac_rows(&source);
        assert!(rows.contains_key("policy") && rows.contains_key("reservations"));

        // An ordinary open refuses the predecessor by name and writes nothing.
        let dir = tmp_dir("grant-upgrade");
        predecessor_with(&dir, &rows);
        let before = std::fs::read(dir.join("rbac.redb")).unwrap();
        let error = open_test_store(&dir).unwrap_err();
        assert!(
            error.to_string().starts_with("RBAC_LAYOUT_OUTDATED"),
            "{error}"
        );
        assert_eq!(std::fs::read(dir.join("rbac.redb")).unwrap(), before);

        // The explicit upgrade keeps every row and creates the empty grant table.
        let staging = private_staging("ok");
        let report =
            super::upgrade_rbac_layout(super::inspect_rbac_layout(&dir, &staging).unwrap())
                .unwrap();
        assert_ne!(report.previous_layout_digest, report.current_layout_digest);
        assert_eq!(rbac_rows(&dir), rows);
        let upgraded = open_test_store(&dir).unwrap();
        assert_eq!(
            (
                upgraded
                    .authority_snapshot()
                    .unwrap()
                    .policy_digest()
                    .to_string(),
                upgraded.reservations().unwrap()
            ),
            expected
        );
        assert!(upgraded.profile_use_grants().unwrap().is_empty());
        drop(upgraded);
        // A file already at the current layout is not a predecessor.
        assert!(super::inspect_rbac_layout(&dir, &staging).is_err());

        // A file that changes after its inspection refuses and stays a predecessor.
        let changed = tmp_dir("grant-changed");
        predecessor_with(&changed, &rows);
        let inspected = super::inspect_rbac_layout(&changed, &staging).unwrap();
        medulla_storage::write_fixture_rows(
            &changed.join("rbac.redb"),
            super::RBAC_TABLE,
            &[("identities", b"{}".as_slice())],
        )
        .unwrap();
        let error = super::upgrade_rbac_layout(inspected)
            .err()
            .expect("refused");
        assert!(
            error.to_string().starts_with("RBAC_UPGRADE_SOURCE_CHANGED"),
            "{error}"
        );
        assert!(open_test_store(&changed)
            .unwrap_err()
            .to_string()
            .starts_with("RBAC_LAYOUT_OUTDATED"));
        std::fs::remove_dir_all(staging).ok();
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.106
    #[test]
    fn profile_use_grants_are_durable_and_revoke_once() {
        use medulla_types::reservation_machine::ReservationEvent;
        let dir = tmp_dir("grants");
        let store = open_test_store(&dir).unwrap();
        let grant = store
            .grant_profile_use("caller-a", "profile-1", 10)
            .unwrap();
        assert!(grant.is_active() && grant.premise().starts_with("profile_grant:"));
        // An exact repeat stores nothing new; another principal gets its own grant.
        let version = store.current_version().unwrap();
        assert_eq!(
            store
                .grant_profile_use("caller-a", "profile-1", 10)
                .unwrap(),
            grant
        );
        assert_eq!(store.current_version().unwrap(), version);
        let other = store
            .grant_profile_use("caller-b", "profile-1", 11)
            .unwrap();
        assert_ne!(other.grant_id, grant.grant_id);

        // Grants survive a reopen; revoking is the one machine edge and happens once.
        drop(store);
        let store = open_test_store(&dir).unwrap();
        assert_eq!(store.profile_use_grants().unwrap().len(), 2);
        let revoked = store.revoke_profile_use(&grant.grant_id, 20).unwrap();
        assert_eq!(
            revoked.state,
            medulla_types::profile_grant_machine::ProfileGrantState::Revoked
        );
        assert_eq!(revoked.state_changed_at_ms, 20);
        let error = store.revoke_profile_use(&grant.grant_id, 21).unwrap_err();
        assert!(error.to_string().contains("TRANSITION_REFUSED"), "{error}");
        let error = store.revoke_profile_use("grant:none", 21).unwrap_err();
        assert!(
            error.to_string().starts_with("RBAC_GRANT_UNKNOWN"),
            "{error}"
        );
        let held = store.profile_use_grants().unwrap();
        assert_eq!(held.iter().filter(|held| held.is_active()).count(), 1);

        // A reservation freezes the image, so a new grant is refused while it is held.
        let revision = store.authority_snapshot().unwrap().revision();
        store.reserve(revision, "txn-g").unwrap();
        let error = store
            .grant_profile_use("caller-c", "profile-1", 30)
            .unwrap_err();
        assert!(error.to_string().starts_with("RBAC_RESERVED"), "{error}");
        store
            .release("txn-g", Some(ReservationEvent::Abort))
            .unwrap();
        store
            .grant_profile_use("caller-c", "profile-1", 30)
            .unwrap();
    }
}
