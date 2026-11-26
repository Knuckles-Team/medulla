//! The logical-replication slot row and its reserved namespace
//! (MEDULLA-DURABLE-KERNEL-R035.7.1).
//!
//! A slot is one row under the reserved key-value namespace
//! [`REPLICATION_SLOT_NAMESPACE`], keyed `<tenant>/<name>`. Only the replication
//! connection of MEDULLA-DURABLE-KERNEL-R035.7.2 writes it, through the dispatch seam
//! under the engine-internal principal; a served `KvPut`, `KvCas`, `KvDelete` or
//! `KvDeclareNamespace` that names the namespace is refused.

use serde::{Deserialize, Serialize};

/// The reserved key-value namespace that holds one row per replication slot.
pub const REPLICATION_SLOT_NAMESPACE: &str = "__pg_replication_slots__";

/// The fixed engine service the replication connection writes slot rows as. Its verified agent id
/// is [`REPLICATION_SLOT_AGENT`]; the KV dispatch admits a write to
/// [`REPLICATION_SLOT_NAMESPACE`] from that agent alone.
pub const REPLICATION_SLOT_SERVICE: &str = "replication-slots";

/// The agent id of the fixed service identity of [`REPLICATION_SLOT_SERVICE`]
/// (`service:<service>`, the shape of every fixed engine service actor).
pub const REPLICATION_SLOT_AGENT: &str = "service:replication-slots";

/// The refusal text prefix of a slot name outside `[a-z0-9_]{1,63}`.
pub const REPLICATION_SLOT_NAME_INVALID: &str = "REPLICATION_SLOT_NAME_INVALID";

/// The longest slot name, in bytes.
const MAX_SLOT_NAME_BYTES: usize = 63;

/// One logical-replication slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplicationSlotRow {
    /// The slot name: 1 to 63 bytes of `[a-z0-9_]`.
    pub name: String,
    /// The tenant that owns the slot.
    pub tenant: String,
    /// The output plugin; `pgoutput`.
    pub plugin: String,
    /// The change-feed cursor the consumer last confirmed; `None` before the first confirm.
    pub confirmed_cursor: Option<String>,
}

/// Whether `name` is a valid slot name.
fn name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SLOT_NAME_BYTES
        && name
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// The key-value key of a slot: `<tenant>/<name>`. A name outside `[a-z0-9_]{1,63}`
/// refuses with the text `REPLICATION_SLOT_NAME_INVALID`.
pub fn slot_key(tenant: &str, name: &str) -> Result<String, String> {
    if !name_is_valid(name) {
        return Err(format!(
            "{REPLICATION_SLOT_NAME_INVALID}: a slot name is 1 to {MAX_SLOT_NAME_BYTES} bytes of [a-z0-9_]"
        ));
    }
    Ok(format!("{tenant}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.1
    #[test]
    fn slot_row_round_trips_and_names_are_bounded() {
        let row = ReplicationSlotRow {
            name: "orders_feed".to_string(),
            tenant: "tenant-a".to_string(),
            plugin: "pgoutput".to_string(),
            confirmed_cursor: Some("cursor-1".to_string()),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert_eq!(serde_json::from_str::<ReplicationSlotRow>(&json).unwrap(), row);
        let packed = rmp_serde::to_vec_named(&row).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ReplicationSlotRow>(&packed).unwrap(),
            row
        );
        let fresh = ReplicationSlotRow {
            confirmed_cursor: None,
            ..row
        };
        let json = serde_json::to_string(&fresh).unwrap();
        assert_eq!(serde_json::from_str::<ReplicationSlotRow>(&json).unwrap(), fresh);
        assert!(serde_json::from_str::<ReplicationSlotRow>(
            r#"{"name":"a","tenant":"t","plugin":"pgoutput","confirmed_cursor":null,"extra":1}"#
        )
        .is_err());

        assert_eq!(slot_key("tenant-a", "orders_feed").unwrap(), "tenant-a/orders_feed");
        assert_eq!(slot_key("t", "a").unwrap(), "t/a");
        assert!(slot_key("t", &"a".repeat(63)).is_ok());
        for bad in ["", "Upper", "has-dash", "has space", "slash/name", "dot.name"] {
            let refusal = slot_key("t", bad).unwrap_err();
            assert!(refusal.starts_with(REPLICATION_SLOT_NAME_INVALID), "{bad}: {refusal}");
        }
        assert!(slot_key("t", &"a".repeat(64))
            .unwrap_err()
            .starts_with(REPLICATION_SLOT_NAME_INVALID));
        assert_eq!(REPLICATION_SLOT_NAMESPACE, "__pg_replication_slots__");
    }
}
