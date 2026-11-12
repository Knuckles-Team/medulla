//! MEDULLA-DECISION-ENGINE-R036.103: the durable RBAC read reservation.
//!
//! A cross-owner admission reserves the authorization image it read, so the policy and the
//! identities cannot change between the read and the coordinator decision. The reservation
//! lives under the `reservations` key of the `rbac` table, beside `policy`, `identities` and
//! `bootstrap`. While any reservation is `Reserved`, every checked save refuses with
//! `RBAC_RESERVED`. A reservation is released only by the durable coordinator decision, and its
//! lifecycle is the shared `ReservationMachine` (`medulla_types::reservation_machine`). The
//! reservation grants no profile access: it only freezes the image.

use std::collections::BTreeMap;

use medulla_types::reservation_machine::{ReservationEvent, ReservationState};
use redb::ReadableTable;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::durable_write::commit_authority_mutation;
use super::{
    authority_identity_digest, authority_policy_digest, RbacPersistError, RbacStore, RBAC_TABLE,
};

/// The `rbac` table key that holds every reservation.
pub(super) const RESERVATIONS_KEY: &str = "reservations";

/// A write was refused because a reservation holds the authorization image.
pub const RBAC_RESERVED: &str = "RBAC_RESERVED";
/// The reserve call named a revision that is no longer current.
pub const RBAC_REVISION_STALE: &str = "RBAC_REVISION_STALE";
/// A release named a transaction that holds no reservation.
pub const RBAC_RESERVATION_UNKNOWN: &str = "RBAC_RESERVATION_UNKNOWN";

/// Every refusal code this module emits (registered in the `commit` family).
pub const REFUSALS: &[&str] = &[RBAC_RESERVED, RBAC_REVISION_STALE, RBAC_RESERVATION_UNKNOWN];

/// One reservation of the authorization image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationRecord {
    /// The coordinator transaction that holds the reservation.
    pub txn_id: String,
    /// The image revision that was read and reserved.
    pub revision: u64,
    /// The canonical digest of the reserved policy.
    pub policy_digest: String,
    /// The canonical digest of the reserved identity map.
    pub identity_digest: String,
    /// The reservation lifecycle: the one persisted state field.
    pub state: ReservationState,
}

impl ReservationRecord {
    /// The evidence premise a commit record lists: `rbac_revision:<n>`.
    pub fn premise(&self) -> String {
        format!("rbac_revision:{}", self.revision)
    }
}

type Reservations = BTreeMap<String, ReservationRecord>;

fn read_reservations(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<Reservations, RbacPersistError> {
    match table
        .get(RESERVATIONS_KEY)
        .map_err(|error| RbacPersistError::Redb(error.to_string()))?
    {
        Some(value) => Ok(serde_json::from_slice(value.value())?),
        None => Ok(Reservations::new()),
    }
}

/// The first open reservation, if any: the check every ordinary write makes.
pub(super) fn require_no_open_reservation(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<(), RbacPersistError> {
    match read_reservations(table)?
        .into_values()
        .find(|record| record.state == ReservationState::Reserved)
    {
        Some(record) => Err(RbacPersistError::Reserved {
            txn_id: record.txn_id,
        }),
        None => Ok(()),
    }
}

fn batch_digest(kind: &str, txn_id: &str) -> String {
    hex::encode(Sha256::digest(format!("{kind}:{txn_id}").as_bytes()))
}

impl RbacStore {
    /// Every reservation, open and resolved, keyed by transaction id.
    pub fn reservations(&self) -> Result<Vec<ReservationRecord>, RbacPersistError> {
        let read = self.scoped_read()?;
        let table = read
            .open_owner_table(RBAC_TABLE)
            .map_err(RbacPersistError::Redb)?;
        Ok(read_reservations(&table)?.into_values().collect())
    }

    /// Reserve the authorization image at `expected_revision` for `txn_id`. The revision
    /// and both digests come from one snapshot. An exact retry returns the held record.
    pub fn reserve(
        &self,
        expected_revision: u64,
        txn_id: &str,
    ) -> Result<ReservationRecord, RbacPersistError> {
        let snapshot = self.authority_snapshot()?;
        if let Some(held) = self
            .reservations()?
            .into_iter()
            .find(|record| record.txn_id == txn_id)
        {
            if held.revision == expected_revision {
                return Ok(held);
            }
        }
        let record = ReservationRecord {
            txn_id: txn_id.to_string(),
            revision: expected_revision,
            policy_digest: authority_policy_digest(snapshot.policy())?,
            identity_digest: authority_identity_digest(snapshot.identities())?,
            state: ReservationState::Reserved,
        };
        let stored = record.clone();
        commit_authority_mutation(
            self,
            expected_revision,
            &batch_digest("reserve", txn_id),
            None,
            |owner_write| {
                let mut table = owner_write
                    .open_table(RBAC_TABLE)
                    .map_err(RbacPersistError::Redb)?;
                require_no_open_reservation(&table)?;
                let mut all = read_reservations(&table)?;
                all.insert(stored.txn_id.clone(), stored.clone());
                table
                    .insert(RESERVATIONS_KEY, serde_json::to_vec(&all)?.as_slice())
                    .map_err(|error| RbacPersistError::Redb(error.to_string()))?;
                Ok(())
            },
        )
        .map_err(|error| match error {
            RbacPersistError::StaleRevision { expected, actual } => {
                RbacPersistError::ReservationStale { expected, actual }
            }
            other => other,
        })?;
        Ok(record)
    }

    /// Resolve the reservation of `txn_id` with the durable coordinator `decision`. A
    /// missing decision leaves the reservation held; a repeat of the applied decision
    /// replays; the opposite decision is refused by `ReservationMachine`.
    pub fn release(
        &self,
        txn_id: &str,
        decision: Option<ReservationEvent>,
    ) -> Result<ReservationRecord, RbacPersistError> {
        let held = self
            .reservations()?
            .into_iter()
            .find(|record| record.txn_id == txn_id)
            .ok_or_else(|| RbacPersistError::ReservationUnknown(txn_id.to_string()))?;
        let Some(event) = decision else {
            return Err(RbacPersistError::Reserved {
                txn_id: txn_id.to_string(),
            });
        };
        let next = held
            .state
            .resolve(event)
            .map_err(RbacPersistError::Machine)?;
        if next == held.state {
            return Ok(held);
        }
        let resolved = ReservationRecord {
            state: next,
            ..held
        };
        let stored = resolved.clone();
        commit_authority_mutation(
            self,
            self.current_version()?,
            &batch_digest(event_name(event), txn_id),
            None,
            |owner_write| {
                let mut table = owner_write
                    .open_table(RBAC_TABLE)
                    .map_err(RbacPersistError::Redb)?;
                let mut all = read_reservations(&table)?;
                all.insert(stored.txn_id.clone(), stored.clone());
                table
                    .insert(RESERVATIONS_KEY, serde_json::to_vec(&all)?.as_slice())
                    .map_err(|error| RbacPersistError::Redb(error.to_string()))?;
                Ok(())
            },
        )?;
        Ok(resolved)
    }
}

fn event_name(event: ReservationEvent) -> &'static str {
    match event {
        ReservationEvent::Commit => "release-commit",
        ReservationEvent::Abort => "release-abort",
    }
}
