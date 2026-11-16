//! Served agent-bus roster model (MEDULLA-DURABLE-KERNEL-R095.1000).
//!
//! A [`BusParticipant`] is one session's row on a tenant's bus. It is stored as
//! the node `bus:participant:<session_ref>` with label `BusParticipant` in the
//! request graph, stamped with the registering principal. Topic messages and
//! hub peers are separate rows (R850, R851); there is no second message model.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Node label of a participant row.
pub const BUS_PARTICIPANT_LABEL: &str = "BusParticipant";
/// Node id prefix of a participant row.
pub const BUS_PARTICIPANT_PREFIX: &str = "bus:participant:";
/// Largest `session_ref`, in bytes.
pub const MAX_SESSION_REF_BYTES: usize = 128;
/// Most capabilities one participant declares.
pub const MAX_CAPABILITIES: usize = 64;
/// Largest single capability, in bytes.
pub const MAX_CAPABILITY_BYTES: usize = 64;
/// Most participants one tenant holds (`Fixed`).
pub const MAX_PARTICIPANTS_PER_TENANT: usize = 10_000;
/// Largest `tenant` or `host_ref`, in bytes.
pub const MAX_REF_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BusParticipant {
    pub session_ref: String,
    pub tenant: String,
    pub host_ref: String,
    pub capabilities: BTreeSet<String>,
    pub last_seen_ms: u64,
}

/// One roster row: the participant, its registering principal and liveness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BusRosterEntry {
    pub participant: BusParticipant,
    pub principal: String,
    pub live: bool,
}

/// The served roster of one tenant, ordered by `session_ref` ascending.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BusRoster {
    pub participants: Vec<BusRosterEntry>,
}

/// The stored node body: the participant plus the registering principal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredParticipant {
    label: String,
    principal: String,
    #[serde(flatten)]
    participant: BusParticipant,
}

/// The node id of one session's participant row.
pub fn participant_node_id(session_ref: &str) -> String {
    format!("{BUS_PARTICIPANT_PREFIX}{session_ref}")
}

impl BusParticipant {
    /// Refuse a participant over the bounds; the error is a `BUS_FIELD_TOO_LARGE` detail.
    pub fn check_bounds(&self) -> Result<(), String> {
        if self.session_ref.is_empty() || self.session_ref.len() > MAX_SESSION_REF_BYTES {
            return Err(format!(
                "session_ref must be 1..={MAX_SESSION_REF_BYTES} bytes"
            ));
        }
        if self.tenant.len() > MAX_REF_BYTES || self.host_ref.len() > MAX_REF_BYTES {
            return Err(format!(
                "tenant and host_ref must be at most {MAX_REF_BYTES} bytes"
            ));
        }
        if self.capabilities.len() > MAX_CAPABILITIES {
            return Err(format!("at most {MAX_CAPABILITIES} capabilities"));
        }
        if self
            .capabilities
            .iter()
            .any(|c| c.len() > MAX_CAPABILITY_BYTES)
        {
            return Err(format!(
                "a capability must be at most {MAX_CAPABILITY_BYTES} bytes"
            ));
        }
        Ok(())
    }

    /// Encode the participant node body (MessagePack JSON object).
    pub fn to_node_props(&self, principal: &str) -> Vec<u8> {
        let stored = StoredParticipant {
            label: BUS_PARTICIPANT_LABEL.to_string(),
            principal: principal.to_string(),
            participant: self.clone(),
        };
        let value = serde_json::to_value(&stored).unwrap_or(serde_json::Value::Null);
        rmp_serde::to_vec_named(&value).unwrap_or_default()
    }

    /// The node body after a heartbeat at `now_ms`: one `last_seen_ms` upsert.
    pub fn heartbeat_props(bytes: &[u8], now_ms: u64) -> Option<Vec<u8>> {
        let (mut participant, principal) = Self::from_node_props(bytes)?;
        participant.last_seen_ms = now_ms;
        Some(participant.to_node_props(&principal))
    }

    /// Decode a participant node body into the participant and its principal.
    pub fn from_node_props(bytes: &[u8]) -> Option<(Self, String)> {
        let value: serde_json::Value = rmp_serde::from_slice(bytes).ok()?;
        let stored: StoredParticipant = serde_json::from_value(value).ok()?;
        (stored.label == BUS_PARTICIPANT_LABEL).then_some((stored.participant, stored.principal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn participant() -> BusParticipant {
        BusParticipant {
            session_ref: "s-1".into(),
            tenant: "t".into(),
            host_ref: "h".into(),
            capabilities: ["plan".to_string(), "code".to_string()].into(),
            last_seen_ms: 42,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R095.1000
    #[test]
    fn participant_round_trips_and_bounds() {
        let p = participant();
        let packed = rmp_serde::to_vec_named(&p).unwrap();
        assert_eq!(rmp_serde::from_slice::<BusParticipant>(&packed).unwrap(), p);
        let props = p.to_node_props("alice");
        assert_eq!(
            BusParticipant::from_node_props(&props),
            Some((p.clone(), "alice".to_string()))
        );
        assert_eq!(participant_node_id("s-1"), "bus:participant:s-1");
        assert!(p.check_bounds().is_ok());

        let json = serde_json::json!({"session_ref": "s", "tenant": "t", "host_ref": "h",
            "capabilities": [], "last_seen_ms": 0, "extra": 1});
        assert!(serde_json::from_value::<BusParticipant>(json).is_err());

        let mut long = p.clone();
        long.session_ref = "x".repeat(MAX_SESSION_REF_BYTES + 1);
        assert!(long.check_bounds().is_err());
        let mut many = p.clone();
        many.capabilities = (0..=MAX_CAPABILITIES).map(|i| format!("c{i}")).collect();
        assert!(many.check_bounds().is_err());
        let mut wide = p;
        wide.capabilities = ["y".repeat(MAX_CAPABILITY_BYTES + 1)].into();
        assert!(wide.check_bounds().is_err());
    }
}
