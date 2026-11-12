//! MEDULLA-DECISION-ENGINE-R036.104: the Agent Library as a durable participant
//! in a cross-owner admission.
//!
//! `AgentLibraryStore::reserve_admission` writes a sealed [`AdmissionIntent`],
//! keyed by its `txn_id`, into the Agent Library redb and fences every component
//! write that would change a component the intent names. The fence stays until
//! the coordinator decision arrives (`resolve_admission`); there is no timeout
//! release. The reservation, its fences and the mutation receipt are written in
//! ONE owner transaction.
//!
//! The reservation is the explicit `ReservationMachine`
//! (`medulla_types::reservation_machine`): `Reserved` is the initial state,
//! `Commit` enters `Committed` and `Abort` enters `Aborted`; both are terminal.
//! A refused event is a typed `TRANSITION_REFUSED` naming the machine, the
//! current state and the event. The state is one persisted field of the
//! [`AdmissionRow`].

use medulla_storage::{AGENT_COMPONENT_HEADS, AGENT_COMPONENT_REVISIONS, DECISION_ARTIFACTS};
pub use medulla_types::reservation_machine::{
    ReservationEvent as AdmissionEvent, ReservationState as AdmissionState, RESERVATION_MACHINE,
};
use redb::ReadableTable;
use serde::{Deserialize, Serialize};

use super::AgentLibraryStore;
use crate::agent_component::ComponentLayer;
use crate::agent_revision::decode_revision;
use crate::decision_jobs::{decode_artifact, encode_artifact};
use medulla_agent_types::agent_library::AgentLibraryLifecycle;

/// The component a reservation names changed since the intent was sealed.
pub const ADMISSION_HEAD_CHANGED: &str = "ADMISSION_HEAD_CHANGED";
/// The component a reservation names is absent or no longer served.
pub const ADMISSION_COMPONENT_WITHDRAWN: &str = "ADMISSION_COMPONENT_WITHDRAWN";
/// The `txn_id` already holds a different intent.
pub const ADMISSION_INTENT_CHANGED: &str = "ADMISSION_INTENT_CHANGED";
/// Another reservation holds the write key, or a reservation fences the component.
pub const ADMISSION_WRITE_BLOCKED: &str = "ADMISSION_WRITE_BLOCKED";
/// The reservation was aborted: a new attempt needs a new `txn_id`.
pub const ADMISSION_ABORTED: &str = "ADMISSION_ABORTED";
/// No reservation exists for the `txn_id`.
pub const ADMISSION_NOT_RESERVED: &str = "ADMISSION_NOT_RESERVED";

/// Every refusal code this module emits.
pub const REFUSALS: &[&str] = &[
    ADMISSION_HEAD_CHANGED,
    ADMISSION_COMPONENT_WITHDRAWN,
    ADMISSION_INTENT_CHANGED,
    ADMISSION_WRITE_BLOCKED,
    ADMISSION_ABORTED,
    ADMISSION_NOT_RESERVED,
];

/// A component head a reservation is sealed over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentHead {
    /// The component id.
    pub component_id: String,
    /// The revision that was the head when the intent was sealed.
    pub entry_revision: u64,
    /// That revision's definition digest.
    pub definition_digest: String,
}

/// The sealed intent of one cross-owner admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionIntent {
    /// The coordinator transaction id.
    pub txn_id: String,
    /// The policy profile revision the admission was decided under.
    pub profile_revision: String,
    /// Every component head the admission rests on, sorted by component id.
    pub component_heads: Vec<ComponentHead>,
    /// The digest of the candidate scope the admitting party saw.
    pub visibility_digest: String,
    /// The key of the one write this admission protects.
    pub write_key: String,
}

impl AdmissionIntent {
    /// The evidence premises of the intent: `component_head:<id>=<digest>`.
    pub fn premises(&self) -> Vec<String> {
        self.component_heads
            .iter()
            .map(|head| {
                format!(
                    "component_head:{}={}",
                    head.component_id, head.definition_digest
                )
            })
            .collect()
    }
}

/// The persisted reservation: the sealed intent and its one state field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRow {
    /// The sealed intent.
    pub intent: AdmissionIntent,
    /// The machine state.
    pub state: AdmissionState,
}

fn admission_key(txn_id: &str) -> String {
    format!("admission:{txn_id}")
}

fn write_key_row(write_key: &str) -> String {
    format!("admission-write:{write_key}")
}

fn fence_key(component_id: &str) -> String {
    format!("admission-fence:{component_id}")
}

type Rows<'a> = redb::Table<'a, (&'static str, &'static str), &'static [u8]>;

fn read_row(rows: &Rows<'_>, tenant: &str, key: &str) -> Result<Option<Vec<u8>>, String> {
    Ok(rows
        .get((tenant, key))
        .map_err(|error| error.to_string())?
        .map(|row| row.value().to_vec()))
}

fn put_row(rows: &mut Rows<'_>, tenant: &str, key: &str, bytes: &[u8]) -> Result<(), String> {
    rows.insert((tenant, key), bytes)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn fence_holders(rows: &Rows<'_>, tenant: &str, component_id: &str) -> Result<Vec<String>, String> {
    match read_row(rows, tenant, &fence_key(component_id))? {
        Some(bytes) => decode_artifact(&bytes, "admission fence"),
        None => Ok(Vec::new()),
    }
}

fn store_fence(
    rows: &mut Rows<'_>,
    tenant: &str,
    component_id: &str,
    holders: &[String],
) -> Result<(), String> {
    let key = fence_key(component_id);
    if holders.is_empty() {
        rows.remove((tenant, key.as_str()))
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    put_row(rows, tenant, &key, &encode_artifact(&holders)?)
}

/// Refuse a component write while a reservation fences `component_id`. Called
/// inside every component write's admitting transaction.
pub(crate) fn require_unfenced(
    txn: &medulla_transaction::AdmittedMutation<'_, medulla_storage::AgentLibraryOwner>,
    tenant: &str,
    component_id: &str,
) -> Result<(), String> {
    let rows = txn.open_read_table(DECISION_ARTIFACTS)?;
    let Some(row) = rows.get((tenant, fence_key(component_id).as_str()))? else {
        return Ok(());
    };
    let holders: Vec<String> = decode_artifact(row.value(), "admission fence")?;
    if holders.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{ADMISSION_WRITE_BLOCKED}: component '{component_id}' is fenced by admission {}",
        holders.join(", ")
    ))
}

impl AgentLibraryStore {
    /// Reserve `intent` for `tenant_id`. Returns the reservation's state: an
    /// exact retry of a live or committed intent succeeds; a changed intent for
    /// the same `txn_id` refuses.
    pub fn reserve_admission(
        &self,
        tenant_id: &str,
        intent: &AdmissionIntent,
    ) -> Result<AdmissionState, String> {
        let mut outcome = None;
        self.maintain_control_rows(tenant_id, "admission_reserve", |owner| {
            let mut tables = owner.open_table(DECISION_ARTIFACTS)?;
            let key = admission_key(&intent.txn_id);
            if let Some(bytes) = read_row(&tables, tenant_id, &key)? {
                let row: AdmissionRow = decode_artifact(&bytes, "admission")?;
                if row.intent != *intent {
                    return Err(format!(
                        "{ADMISSION_INTENT_CHANGED}: txn '{}' holds a different intent",
                        intent.txn_id
                    ));
                }
                if row.state == AdmissionState::Aborted {
                    return Err(format!(
                        "{ADMISSION_ABORTED}: txn '{}' was aborted; start a new txn",
                        intent.txn_id
                    ));
                }
                outcome = Some(row.state);
                return Ok(());
            }
            if let Some(holder) = read_row(&tables, tenant_id, &write_key_row(&intent.write_key))? {
                let holder: String = decode_artifact(&holder, "admission write key")?;
                let live = read_row(&tables, tenant_id, &admission_key(&holder))?
                    .map(|bytes| decode_artifact::<AdmissionRow>(&bytes, "admission"))
                    .transpose()?
                    .is_some_and(|row| row.state == AdmissionState::Reserved);
                if live {
                    return Err(format!(
                        "{ADMISSION_WRITE_BLOCKED}: write key '{}' is held by txn '{holder}'",
                        intent.write_key
                    ));
                }
            }
            let heads = owner.open_table(AGENT_COMPONENT_HEADS)?;
            let revisions = owner.open_table(AGENT_COMPONENT_REVISIONS)?;
            for head in &intent.component_heads {
                let id = head.component_id.as_str();
                let current = heads
                    .get((tenant_id, id))
                    .map_err(|error| error.to_string())?
                    .map(|row| row.value());
                let entry = match current {
                    Some(revision) => revisions
                        .get((tenant_id, id, revision))
                        .map_err(|error| error.to_string())?
                        .map(|row| decode_revision::<ComponentLayer>(row.value()))
                        .transpose()?,
                    None => None,
                };
                let Some(entry) = entry.filter(|e| e.lifecycle == AgentLibraryLifecycle::Published)
                else {
                    return Err(format!(
                        "{ADMISSION_COMPONENT_WITHDRAWN}: component '{id}' is absent or no longer served"
                    ));
                };
                if entry.entry_revision != head.entry_revision
                    || entry.definition_digest != head.definition_digest
                {
                    return Err(format!(
                        "{ADMISSION_HEAD_CHANGED}: component '{id}' is at revision {} not {}",
                        entry.entry_revision, head.entry_revision
                    ));
                }
            }
            for head in &intent.component_heads {
                let mut holders = fence_holders(&tables, tenant_id, &head.component_id)?;
                if !holders.contains(&intent.txn_id) {
                    holders.push(intent.txn_id.clone());
                }
                store_fence(&mut tables, tenant_id, &head.component_id, &holders)?;
            }
            put_row(
                &mut tables,
                tenant_id,
                &write_key_row(&intent.write_key),
                &encode_artifact(&intent.txn_id)?,
            )?;
            let row = AdmissionRow {
                intent: intent.clone(),
                state: RESERVATION_MACHINE.initial,
            };
            put_row(&mut tables, tenant_id, &key, &encode_artifact(&row)?)?;
            outcome = Some(row.state);
            Ok(())
        })?;
        outcome.ok_or_else(|| "admission reservation produced no state".to_string())
    }

    /// Apply the coordinator decision `event` to the reservation `txn_id` and
    /// release its fences. A repeat of the decision already applied replays;
    /// the opposite decision is refused by `AdmissionMachine`.
    pub fn resolve_admission(
        &self,
        tenant_id: &str,
        txn_id: &str,
        event: AdmissionEvent,
    ) -> Result<AdmissionState, String> {
        let mut outcome = None;
        self.maintain_control_rows(tenant_id, "admission_resolve", |owner| {
            let mut tables = owner.open_table(DECISION_ARTIFACTS)?;
            let key = admission_key(txn_id);
            let Some(bytes) = read_row(&tables, tenant_id, &key)? else {
                return Err(format!(
                    "{ADMISSION_NOT_RESERVED}: txn '{txn_id}' has no reservation"
                ));
            };
            let mut row: AdmissionRow = decode_artifact(&bytes, "admission")?;
            if row.state == event.target() {
                outcome = Some(row.state);
                return Ok(());
            }
            row.state = row
                .state
                .resolve(event)
                .map_err(|refused| refused.to_string())?;
            for head in &row.intent.component_heads {
                let mut holders = fence_holders(&tables, tenant_id, &head.component_id)?;
                holders.retain(|holder| holder != txn_id);
                store_fence(&mut tables, tenant_id, &head.component_id, &holders)?;
            }
            let write_row = write_key_row(&row.intent.write_key);
            if let Some(holder) = read_row(&tables, tenant_id, &write_row)? {
                if decode_artifact::<String>(&holder, "admission write key")? == txn_id {
                    tables
                        .remove((tenant_id, write_row.as_str()))
                        .map_err(|error| error.to_string())?;
                }
            }
            put_row(&mut tables, tenant_id, &key, &encode_artifact(&row)?)?;
            outcome = Some(row.state);
            Ok(())
        })?;
        outcome.ok_or_else(|| "admission resolution produced no state".to_string())
    }

    /// The reservation `txn_id`, if one exists.
    pub fn admission(&self, tenant_id: &str, txn_id: &str) -> Result<Option<AdmissionRow>, String> {
        self.decision_artifact(tenant_id, &admission_key(txn_id))?
            .map(|bytes| decode_artifact(&bytes, "admission"))
            .transpose()
    }
}
