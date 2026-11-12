//! Sync state as graph facts (MEDULLA-SRC-AUTH-R012.4).

use serde_json::{json, Map, Value};

use super::{SyncAction, SyncMutation};
use medulla_types::contract::Digest256;

/// The `vg:SyncState` class IRI.
pub const SYNC_STATE_CLASS: &str = "https://knuckles-team.github.io/ontology/virtual-graph#SyncState";
/// Domain separator of the sync-state node id.
const NODE_DOMAIN: &str = "eg/source-sync-state/v1";

/// What one record's sync decided, with the digests the state keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub record_key: String,
    pub action: SyncAction,
    /// The base digest before this sync (`None` for a record never synced).
    pub base_digest: Option<String>,
    /// The source digest read in this sync (`None` when absent there).
    pub source_digest: Option<String>,
    pub source_revision: Option<String>,
}

fn node_id(connection: &str, class: &str, record_key: &str) -> String {
    let mut framed = Vec::new();
    for part in [NODE_DOMAIN, connection, class, record_key] {
        framed.extend_from_slice(part.as_bytes());
        framed.push(0);
    }
    format!("sync-state:{}", Digest256::sha256(&framed).to_hex())
}

/// One `vg:SyncState` upsert per applied or proposed record, keyed by
/// connection, class and record key. An applied record's base becomes the
/// source digest it took; a proposed record keeps its earlier base, so the next
/// sync still sees the graph change until the source reflects it. Unchanged
/// records and held conflicts write nothing.
pub fn sync_state_mutations(
    connection: &str,
    class: &str,
    outcomes: &[SyncOutcome],
    now_ms: u64,
) -> Vec<SyncMutation> {
    outcomes
        .iter()
        .filter_map(|outcome| {
            let base = match &outcome.action {
                SyncAction::ApplyInbound => outcome.source_digest.clone(),
                action if action.proposes_outbound() => outcome.base_digest.clone(),
                _ => return None,
            };
            let mut properties = Map::new();
            let mut put = |key: &str, value: Value| {
                properties.insert(key.to_string(), value);
            };
            put("type", json!(SYNC_STATE_CLASS));
            put("syncConnection", json!(connection));
            put("syncClass", json!(class));
            put("syncRecordKey", json!(outcome.record_key));
            if let Some(base) = base {
                put("baseDigest", json!(base));
            }
            if let Some(revision) = &outcome.source_revision {
                put("sourceRevision", json!(revision));
            }
            put("syncedAtMs", json!(now_ms));
            Some(SyncMutation {
                node_id: node_id(connection, class, &outcome.record_key),
                properties,
            })
        })
        .collect()
}
