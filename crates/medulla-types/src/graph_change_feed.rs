//! Graph change-feed types and snapshot digest (MEDULLA-DURABLE-KERNEL-R043.100.1).
//!
//! `OpenGraphSnapshot` (R043.100.2) answers a [`GraphSnapshot`] whose
//! `high_water_cursor` resumes `ReadGraphChanges` (R043.100.3), which pages
//! [`GraphChangeEvent`]s. A cursor is opaque: it is the tenant-bound
//! [`CursorFamily`] shape over a resume key that binds graph, incarnation,
//! authorization revision and last event, so it cannot be replayed against
//! another tenant, graph, incarnation or authorization revision. Digests are
//! SHA-256, lowercase hex.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::tenant_cursor::CursorFamily;

/// Wire schema version of the feed types.
pub const GRAPH_CHANGE_FEED_SCHEMA_VERSION: u32 = 1;

/// The change-feed cursor family.
pub const GRAPH_CHANGE_CURSOR: CursorFamily = CursorFamily {
    domain: b"medulla.graph_change_feed.v1",
    max_bytes: 4_096,
    noun: "graph change feed",
};

/// One consistent snapshot and the cursor that continues after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSnapshot {
    pub schema_version: u32,
    pub incarnation: u64,
    pub committed_revision: u64,
    /// SHA-256 of the snapshot body, lowercase hex.
    pub snapshot_digest: String,
    pub visibility_revision: u64,
    pub high_water_cursor: String,
}

/// What one committed event changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphChange {
    /// Nodes and edges written by the commit.
    Upsert {
        node_ids: Vec<String>,
        edge_ids: Vec<String>,
    },
    /// Ids deleted by the commit and the revision that deleted them.
    Tombstone {
        deleted_ids: Vec<String>,
        deletion_revision: u64,
    },
}

/// One committed change, delivered at least once under a stable id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphChangeEvent {
    pub schema_version: u32,
    pub commit_id: String,
    pub event_ordinal: u64,
    pub committed_at_ms: u64,
    pub visible_at_ms: u64,
    pub latency_profile_revision: u64,
    pub commit_to_visible_ms: u64,
    pub change: GraphChange,
}

/// The scope and position a change-feed cursor binds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphChangeCursor {
    pub graph: String,
    pub incarnation: u64,
    pub authorization_revision: u64,
    pub last_event: u64,
}

impl GraphChangeCursor {
    /// Mint the opaque cursor for `tenant`.
    pub fn encode(&self, tenant: &str) -> String {
        let key = serde_json::to_string(self).unwrap_or_default();
        GRAPH_CHANGE_CURSOR.encode(tenant, &key)
    }

    /// Recover the bound scope, refusing a cursor minted for another tenant
    /// or one that does not decode.
    pub fn decode(tenant: &str, cursor: &str) -> Result<Self, String> {
        let key = GRAPH_CHANGE_CURSOR.decode(tenant, cursor, |key| {
            serde_json::from_str::<GraphChangeCursor>(key).is_ok()
        })?;
        serde_json::from_str(&key).map_err(|error| format!("graph change feed cursor: {error}"))
    }
}

/// SHA-256 digest of a snapshot body, lowercase hex.
pub fn snapshot_digest(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R043.100.1
    #[test]
    fn types_round_trip_and_cursor_binds_scope() {
        let cursor = GraphChangeCursor {
            graph: "g|1".into(),
            incarnation: 3,
            authorization_revision: 7,
            last_event: 42,
        };
        let token = cursor.encode("tenant-a");
        assert_eq!(
            GraphChangeCursor::decode("tenant-a", &token),
            Ok(cursor.clone())
        );
        assert!(GraphChangeCursor::decode("tenant-b", &token).is_err());
        for other in [
            GraphChangeCursor {
                graph: "g|2".into(),
                ..cursor.clone()
            },
            GraphChangeCursor {
                incarnation: 4,
                ..cursor.clone()
            },
            GraphChangeCursor {
                authorization_revision: 8,
                ..cursor.clone()
            },
            GraphChangeCursor {
                last_event: 43,
                ..cursor.clone()
            },
        ] {
            assert_ne!(other.encode("tenant-a"), token, "every field is bound");
        }
        assert!(GraphChangeCursor::decode("tenant-a", "zz").is_err());

        let digest = snapshot_digest(b"abc");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let snapshot = GraphSnapshot {
            schema_version: GRAPH_CHANGE_FEED_SCHEMA_VERSION,
            incarnation: 3,
            committed_revision: 9,
            snapshot_digest: digest,
            visibility_revision: 9,
            high_water_cursor: token,
        };
        let back: GraphSnapshot =
            serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();
        assert_eq!(back, snapshot);
        for change in [
            GraphChange::Upsert {
                node_ids: vec!["n".into()],
                edge_ids: vec!["e".into()],
            },
            GraphChange::Tombstone {
                deleted_ids: vec!["n".into()],
                deletion_revision: 10,
            },
        ] {
            let event = GraphChangeEvent {
                schema_version: GRAPH_CHANGE_FEED_SCHEMA_VERSION,
                commit_id: "c1".into(),
                event_ordinal: 1,
                committed_at_ms: 1_000,
                visible_at_ms: 1_005,
                latency_profile_revision: 1,
                commit_to_visible_ms: 5,
                change,
            };
            let back: GraphChangeEvent =
                serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
            assert_eq!(back, event);
        }
    }
}
