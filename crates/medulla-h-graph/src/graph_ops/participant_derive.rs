//! The graph side of derivation enqueueing (MEDULLA-DECISION-ENGINE-R651.11).
//!
//! After a turn commits, the queue side (`medulla-server-graph` `memory_derive::enqueue_derivations`)
//! needs graph reads and one node write, all keyed by the verified tenant scope: the turns of a
//! session, the stored observation settings of its members, and the `DerivePayload` node of each
//! derivation key. This file holds exactly those, so the queue side never decodes a stored node
//! itself.

use super::assistant_profile::node_blob;
use super::*;
use medulla_decision_stats::decision::participant_derive::{
    conclusion_claim_prefix, derivation_targets, derive_held, key_node_digest, select_turns, DerivationInput, DeriveCheckpoint,
    DeriveKey, DerivePayload,
};
use medulla_decision_types::memory_events::{DerivationCompleted, MemoryEvent};
use medulla_decision_types::participant_memory::{DeriveInput, DeriveTurn};
use medulla_work_types::memory_scope::{scope_node_id, MemoryScope};
use medulla_isolation::profile_access::read_stored;
use medulla_work_types::memory_scope::ObservationConfig;
use medulla_work_types::participant::ParticipantId;

pub use super::memory_surface::{session_turns, SessionTurn};

const PAYLOAD_LABEL: &str = "DerivePayload";

/// The stored observation settings of `members` in `session_id`; a member without one is absent
/// from the result (the reader applies the default).
pub fn observation_configs(
    core: &GraphCore,
    tenant: &str,
    session_id: &str,
    members: &[ParticipantId],
) -> Vec<ObservationConfig> {
    members
        .iter()
        .filter_map(|member| super::participant_surface::observation_stored(core, tenant, session_id, member))
        .collect()
}

/// The graph node id of the payload of `key`, keyed by the tenant scope.
pub fn payload_node_id(tenant: &str, key: &DeriveKey) -> String {
    format!("derive_payload:{tenant}:{}", key_node_digest(key))
}

/// The stored payload at `node_id`, or `None` when the tenant has none there.
pub fn read_payload(core: &GraphCore, tenant: &str, node_id: &str) -> Option<DerivePayload> {
    read_stored(core, node_id, PAYLOAD_LABEL, tenant)
}

/// The node upsert that stores `payload` at `node_id`; it commits through the same durable write
/// path as every other node.
pub fn payload_write(tenant: &str, node_id: &str, payload: &DerivePayload) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: node_blob(PAYLOAD_LABEL, tenant, payload)?,
    })
}

const CHECKPOINT_LABEL: &str = "DeriveCheckpoint";
const SCOPE_LABEL: &str = "MemoryScope";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The graph node id of the checkpoint of `key`.
pub fn checkpoint_node_id(tenant: &str, key: &DeriveKey) -> String {
    format!("derive_checkpoint:{tenant}:{}", key_node_digest(key))
}

/// The stored checkpoint of `key`, when a job has completed for it.
pub fn read_checkpoint(core: &GraphCore, tenant: &str, key: &DeriveKey) -> Option<DeriveCheckpoint> {
    read_stored(core, &checkpoint_node_id(tenant, key), CHECKPOINT_LABEL, tenant)
}

/// Every turn of the key's session, in stored order, as the worker reads them.
fn session_view(core: &GraphCore, tenant: &str, session_id: &str) -> Vec<DeriveTurn> {
    session_turns(core, tenant, session_id)
        .into_iter()
        .map(|turn| DeriveTurn {
            turn_id: turn.memory_id,
            speaker: turn.speaker,
            text: turn.text,
            timestamp_ms: turn.timestamp_ms,
        })
        .collect()
}

/// The payload a derive task names, or the one absent envelope.
fn payload_of(core: &GraphCore, tenant: &str, payload_ref: &str) -> Result<DerivePayload, String> {
    read_payload(core, tenant, payload_ref)
        .ok_or_else(|| refuse("DERIVE_PAYLOAD_NOT_FOUND", format!("no derive payload {payload_ref}")))
}

/// Whether the memory scope the key runs under still holds the key's session; a key with no scope
/// always does.
fn scope_holds(core: &GraphCore, tenant: &str, key: &DeriveKey) -> bool {
    if key.scope_id.is_empty() {
        return true;
    }
    read_stored::<MemoryScope>(core, &scope_node_id(tenant, &key.scope_id), SCOPE_LABEL, tenant)
        .is_some_and(|scope| scope.session_ids.contains(&key.session_id))
}

/// Whether the key is still one of its session's derivation targets under the stored observation
/// settings (a setting changed after the task was queued).
fn still_a_target(core: &GraphCore, tenant: &str, key: &DeriveKey, turns: &[DeriveTurn]) -> bool {
    let members: Vec<ParticipantId> = turns
        .iter()
        .map(|turn| turn.speaker.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let configs = observation_configs(core, tenant, &key.session_id, &members);
    derivation_targets(&DerivationInput {
        scope_id: &key.scope_id,
        session_id: &key.session_id,
        members: &members,
        configs: &configs,
    })
    .contains(key)
}

/// `memory.derive_max_conclusions` as a count.
fn max_conclusions() -> u32 {
    u32::try_from(medulla_server_api::knob_config::knobs().memory_derive_max_conclusions).unwrap_or(u32::MAX)
}

/// What a derive worker reads for the task whose payload is `payload_ref`
/// (MEDULLA-DECISION-ENGINE-R651.12): the key's perspective and the subject's turns of the range
/// that the perspective may see. A range that starts after a gap in the checkpoint is
/// `DERIVE_ORDER_HELD`; a payload the tenant does not hold reads as absent.
pub fn derive_input(core: &GraphCore, tenant: &str, payload_ref: &str) -> Result<DeriveInput, String> {
    let payload = payload_of(core, tenant, payload_ref)?;
    let key = &payload.key;
    let session = session_view(core, tenant, &key.session_id);
    let checkpoint = read_checkpoint(core, tenant, key);
    if derive_held(&key.subject, &session, checkpoint.as_ref().map(|c| c.last_turn_id.as_str()), &payload.first_turn_id) {
        return Err(refuse(
            "DERIVE_ORDER_HELD",
            format!("an earlier range of session {} is not derived yet", key.session_id),
        ));
    }
    let allowed = scope_holds(core, tenant, key) && still_a_target(core, tenant, key, &session);
    Ok(DeriveInput {
        scope_id: key.scope_id.clone(),
        session_id: key.session_id.clone(),
        subject: key.subject.clone(),
        observer: key.observer.clone(),
        first_turn_id: payload.first_turn_id.clone(),
        last_turn_id: payload.last_turn_id.clone(),
        turns: select_turns(key, &payload.first_turn_id, &payload.last_turn_id, &session, allowed),
        max_conclusions: max_conclusions(),
        claim_prefix: conclusion_claim_prefix(key),
    })
}

/// The one write that finishes a derive task: the checkpoint node of the key and the
/// `memory.derivation.completed` event node, as one batch (MEDULLA-DECISION-ENGINE-R651.12,
/// R073.6). `None` as the write means the range was already completed. A count above
/// `memory.derive_max_conclusions` is `DERIVE_CONCLUSION_LIMIT`.
pub fn advance_checkpoint(
    core: &GraphCore,
    tenant: &str,
    payload_ref: &str,
    claims_written: u32,
    at_ms: u64,
) -> Result<(Option<Method>, DerivationCompleted), String> {
    let payload = payload_of(core, tenant, payload_ref)?;
    let key = &payload.key;
    if claims_written > max_conclusions() {
        return Err(refuse(
            "DERIVE_CONCLUSION_LIMIT",
            format!("{claims_written} conclusions exceed memory.derive_max_conclusions"),
        ));
    }
    let session = session_view(core, tenant, &key.session_id);
    let checkpoint = read_checkpoint(core, tenant, key);
    if derive_held(&key.subject, &session, checkpoint.as_ref().map(|c| c.last_turn_id.as_str()), &payload.first_turn_id) {
        return Err(refuse(
            "DERIVE_ORDER_HELD",
            format!("an earlier range of session {} is not derived yet", key.session_id),
        ));
    }
    let completed = DerivationCompleted {
        scope_id: key.scope_id.clone(),
        session_id: key.session_id.clone(),
        subject: key.subject.clone(),
        observer: key.observer.clone(),
        claims_written,
        last_turn_id: payload.last_turn_id.clone(),
    };
    if checkpoint.as_ref().is_some_and(|stored| stored.last_turn_id == payload.last_turn_id) {
        return Ok((None, completed));
    }
    let stored = DeriveCheckpoint { last_turn_id: payload.last_turn_id.clone(), completed_at_ms: at_ms };
    let json = serde_json::to_string(&stored).map_err(|error| format!("checkpoint encoding failed: {error}"))?;
    let operations = vec![
        serde_json::json!({
            "op": "upsert_node",
            "id": checkpoint_node_id(tenant, key),
            "properties": { "type": CHECKPOINT_LABEL, "tenant_scope": tenant, "json": json },
        }),
        MemoryEvent::DerivationCompleted(completed.clone()).node_operation(tenant, at_ms)?,
    ];
    let operations_msgpack = rmp_serde::to_vec_named(&serde_json::Value::Array(operations))
        .map_err(|error| format!("checkpoint batch encoding failed: {error}"))?;
    Ok((Some(Method::BatchUpdate { operations_msgpack }), completed))
}

/// Most derive payload nodes one status read scans.
const PAYLOAD_SCAN_CAP: usize = 100_000;

/// Every derive payload the tenant stores, in node-id order.
pub fn stored_payloads(core: &GraphCore, tenant: &str) -> Vec<DerivePayload> {
    let mut ids: Vec<String> = core
        .get_nodes_by_label(PAYLOAD_LABEL, PAYLOAD_SCAN_CAP)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids.into_iter().filter_map(|id| read_payload(core, tenant, &id)).collect()
}
