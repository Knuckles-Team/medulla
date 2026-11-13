//! MEDULLA-DECISION-ENGINE-R036.106: durable profile-use grant records, and the explicit offline
//! upgrade that brings a predecessor RBAC file to the layout that holds them.
//!
//! A grant says that one principal may use one assistant profile. It lives in its own owner
//! table `rbac_profile_use_grant`, keyed by grant id, so it never rides on the authorization
//! image rows. The lifecycle of a grant is the explicit `ProfileGrantMachine`
//! (`medulla_types::profile_grant_machine`): `Active` is initial and `Revoke` enters the
//! terminal `Revoked`; the one persisted `state` field is written only through that machine.
//! A grant never grants more than the use of the named profile: it is not a role, a permission
//! or a reservation.
//!
//! The layout moves from the predecessor to the current one only through [`upgrade_rbac_layout`],
//! an operator action. An ordinary `RbacStore::open` refuses a predecessor file with
//! `RBAC_LAYOUT_OUTDATED` and rewrites nothing.

use std::path::Path;

use medulla_storage::{
    InspectedStoreUpgrade, OfflineUpgradeInspectionOptions, PhysicalStoreIdentity,
    RBAC_BEFORE_PROFILE_GRANTS,
};
use medulla_types::profile_grant_machine::{ProfileGrantEvent, ProfileGrantState};
use redb::{ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::durable_write::commit_authority_mutation;
use super::reservation::require_no_open_reservation;
use super::{RbacPersistError, RbacStore, RBAC_PHYSICAL_STORE, RBAC_TABLE};

/// One grant row per grant id, JSON bytes of a [`ProfileUseGrant`].
const GRANT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("rbac_profile_use_grant");

/// A predecessor file was opened by an ordinary `RbacStore::open`.
pub const RBAC_LAYOUT_OUTDATED: &str = "RBAC_LAYOUT_OUTDATED";
/// The file changed between the inspection and the upgrade write.
pub const RBAC_UPGRADE_SOURCE_CHANGED: &str = "RBAC_UPGRADE_SOURCE_CHANGED";
/// A revoke named a grant that does not exist.
pub const RBAC_GRANT_UNKNOWN: &str = "RBAC_GRANT_UNKNOWN";

/// Every refusal code this module emits (registered in the `commit` family).
pub const REFUSALS: &[&str] = &[
    RBAC_LAYOUT_OUTDATED,
    RBAC_UPGRADE_SOURCE_CHANGED,
    RBAC_GRANT_UNKNOWN,
];

/// One durable permission to use a profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileUseGrant {
    /// Stable id derived from the principal, the profile and the grant time.
    pub grant_id: String,
    /// The principal that may use the profile.
    pub principal: String,
    /// The profile the principal may use.
    pub profile_id: String,
    /// When the grant was made.
    pub granted_at_ms: u64,
    /// The grant lifecycle: the one persisted state field.
    pub state: ProfileGrantState,
    /// When `state` last changed (the grant time while `Active`).
    pub state_changed_at_ms: u64,
}

impl ProfileUseGrant {
    /// Whether the principal may use the profile under this grant right now.
    pub fn is_active(&self) -> bool {
        self.state == ProfileGrantState::Active
    }

    /// The evidence premise a decision lists: `profile_grant:<grant id>`.
    pub fn premise(&self) -> String {
        format!("profile_grant:{}", self.grant_id)
    }
}

fn grant_id(principal: &str, profile_id: &str, granted_at_ms: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(b"profile-use-grant\0");
    hash.update(principal.as_bytes());
    hash.update([0]);
    hash.update(profile_id.as_bytes());
    hash.update([0]);
    hash.update(granted_at_ms.to_be_bytes());
    format!("grant:{}", &hex::encode(hash.finalize())[..32])
}

fn batch_digest(kind: &str, id: &str) -> String {
    hex::encode(Sha256::digest(format!("{kind}:{id}").as_bytes()))
}

fn redb(error: impl ToString) -> RbacPersistError {
    RbacPersistError::Redb(error.to_string())
}

impl RbacStore {
    /// Every grant, active and revoked, ordered by grant id.
    pub fn profile_use_grants(&self) -> Result<Vec<ProfileUseGrant>, RbacPersistError> {
        let read = self.scoped_read()?;
        let table = read
            .open_owner_table(GRANT_TABLE)
            .map_err(RbacPersistError::Redb)?;
        let mut grants = Vec::new();
        for row in table.iter().map_err(redb)? {
            let (_, value) = row.map_err(redb)?;
            grants.push(serde_json::from_slice(value.value())?);
        }
        Ok(grants)
    }

    /// Record that `principal` may use `profile_id`. An exact repeat returns the stored grant.
    /// Refuses with `RBAC_RESERVED` while a reservation holds the authorization image.
    pub fn grant_profile_use(
        &self,
        principal: &str,
        profile_id: &str,
        granted_at_ms: u64,
    ) -> Result<ProfileUseGrant, RbacPersistError> {
        let grant = ProfileUseGrant {
            grant_id: grant_id(principal, profile_id, granted_at_ms),
            principal: principal.to_string(),
            profile_id: profile_id.to_string(),
            granted_at_ms,
            state: ProfileGrantState::Active,
            state_changed_at_ms: granted_at_ms,
        };
        if let Some(stored) = self
            .profile_use_grants()?
            .into_iter()
            .find(|stored| stored.grant_id == grant.grant_id)
        {
            return Ok(stored);
        }
        let row = serde_json::to_vec(&grant)?;
        let key = grant.grant_id.clone();
        commit_authority_mutation(
            self,
            self.current_version()?,
            &batch_digest("grant", &key),
            None,
            |owner_write| {
                require_no_open_reservation(
                    &owner_write
                        .open_table(RBAC_TABLE)
                        .map_err(RbacPersistError::Redb)?,
                )?;
                owner_write
                    .open_table(GRANT_TABLE)
                    .map_err(RbacPersistError::Redb)?
                    .insert(key.as_str(), row.as_slice())
                    .map_err(redb)?;
                Ok(())
            },
        )?;
        Ok(grant)
    }

    /// Withdraw a grant. The one transition is `ProfileGrantMachine` `Active` to `Revoked`;
    /// revoking a revoked grant is refused with `TRANSITION_REFUSED`, an unknown id with
    /// `RBAC_GRANT_UNKNOWN`.
    pub fn revoke_profile_use(
        &self,
        grant_id: &str,
        revoked_at_ms: u64,
    ) -> Result<ProfileUseGrant, RbacPersistError> {
        let held = self
            .profile_use_grants()?
            .into_iter()
            .find(|stored| stored.grant_id == grant_id)
            .ok_or_else(|| RbacPersistError::GrantUnknown(grant_id.to_string()))?;
        let revoked = ProfileUseGrant {
            state: held
                .state
                .apply(ProfileGrantEvent::Revoke)
                .map_err(RbacPersistError::Machine)?,
            state_changed_at_ms: revoked_at_ms,
            ..held
        };
        let row = serde_json::to_vec(&revoked)?;
        let key = revoked.grant_id.clone();
        commit_authority_mutation(
            self,
            self.current_version()?,
            &batch_digest("revoke", &key),
            None,
            |owner_write| {
                owner_write
                    .open_table(GRANT_TABLE)
                    .map_err(RbacPersistError::Redb)?
                    .insert(key.as_str(), row.as_slice())
                    .map_err(redb)?;
                Ok(())
            },
        )?;
        Ok(revoked)
    }
}

/// An RBAC file inspected as a genuine predecessor, bound to the bytes the inspection read.
pub struct InspectedRbacLayout {
    inspected: InspectedStoreUpgrade,
}

/// What one offline layout upgrade did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RbacLayoutUpgradeReport {
    pub previous_layout_digest: [u8; 32],
    pub current_layout_digest: [u8; 32],
}

/// Inspect `{dir}/rbac.redb` without writing it. `staging_root` is the private local directory
/// the bounded inspection uses.
pub fn inspect_rbac_layout(
    dir: &Path,
    staging_root: &Path,
) -> Result<InspectedRbacLayout, RbacPersistError> {
    let upgrade = medulla_storage::registered_offline_upgrade(&RBAC_BEFORE_PROFILE_GRANTS).ok_or(
        RbacPersistError::IncompleteState(
            "no offline upgrade is registered for the predecessor RBAC layout",
        ),
    )?;
    let options = OfflineUpgradeInspectionOptions::new(staging_root.to_path_buf(), 256 << 20)
        .map_err(RbacPersistError::Redb)?;
    let identity =
        PhysicalStoreIdentity::new(RBAC_PHYSICAL_STORE).map_err(RbacPersistError::Redb)?;
    let inspected = upgrade
        .inspect(&dir.join("rbac.redb"), identity, None, options)
        .map_err(RbacPersistError::Redb)?;
    Ok(InspectedRbacLayout { inspected })
}

/// Bring an inspected predecessor RBAC file to the current layout in one durable commit. Every
/// authority and reservation row is kept and the empty grant table is created. A file that
/// changed after its inspection refuses with `RBAC_UPGRADE_SOURCE_CHANGED` and stays a predecessor.
pub fn upgrade_rbac_layout(
    inspected: InspectedRbacLayout,
) -> Result<RbacLayoutUpgradeReport, RbacPersistError> {
    inspected
        .inspected
        .apply()
        .map(|report| RbacLayoutUpgradeReport {
            previous_layout_digest: report.previous_layout_digest,
            current_layout_digest: report.current_layout_digest,
        })
        .map_err(RbacPersistError::UpgradeSourceChanged)
}
