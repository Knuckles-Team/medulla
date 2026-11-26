//! The graph side of derivation enqueueing (MEDULLA-DECISION-ENGINE-R651.11).
//!
//! After a turn commits, the queue side (`medulla-server-graph` `memory_derive::enqueue_derivations`)
//! needs graph reads and one node write, all keyed by the verified tenant scope: the turns of a
//! session, the stored observation settings of its members, and the `DerivePayload` node of each
//! derivation key. This file holds exactly those, so the queue side never decodes a stored node
//! itself.

use super::assistant_profile::node_blob;
use super::*;
use medulla_decision_stats::decision::participant_derive::{key_node_digest, DeriveKey, DerivePayload};
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
