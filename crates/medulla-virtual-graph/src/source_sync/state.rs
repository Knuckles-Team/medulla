//! Sync state as graph facts (MEDULLA-SRC-AUTH-R012.4).

use serde_json::{json, Map, Value};

use super::{SyncAction, SyncMutation};
use medulla_types::contract::Digest256;

/// The `vg:SyncState` class IRI.
pub const SYNC_STATE_CLASS: &str =
    "https://knuckles-team.github.io/ontology/virtual-graph#SyncState";
/// The `vg:SyncConflict` class IRI: a sync state record that also holds both
/// digests of a record both sides changed (MEDULLA-SRC-AUTH-R012.8).
pub const SYNC_CONFLICT_CLASS: &str =
    "https://knuckles-team.github.io/ontology/virtual-graph#SyncConflict";
/// Domain separator of the sync-conflict node id.
const CONFLICT_DOMAIN: &str = "eg/source-sync-conflict/v1";
/// Domain separator of the sync-state node id.
const NODE_DOMAIN: &str = "eg/source-sync-state/v1";

/// What one record's sync decided, with the digests the state keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// The key of the record that was synced.
    pub record_key: String,
    /// The decision taken for that record.
    pub action: SyncAction,
    /// The base digest before this sync (`None` for a record never synced).
    pub base_digest: Option<String>,
    /// The graph value digest read in this sync (`None` when absent there).
    pub graph_digest: Option<String>,
    /// The source digest read in this sync (`None` when absent there).
    pub source_digest: Option<String>,
    /// The source revision read in this sync (`None` when absent there).
    pub source_revision: Option<String>,
}

fn node_id(connection: &str, class: &str, record_key: &str) -> String {
    keyed_id("sync-state", NODE_DOMAIN, connection, class, record_key)
}

fn keyed_id(prefix: &str, domain: &str, connection: &str, class: &str, record_key: &str) -> String {
    let mut framed = Vec::new();
    for part in [domain, connection, class, record_key] {
        framed.extend_from_slice(part.as_bytes());
        framed.push(0);
    }
    format!("{prefix}:{}", Digest256::sha256(&framed).to_hex())
}

/// One `vg:SyncConflict` upsert per record both sides changed, keyed by
/// connection, class and record key, holding both digests and the rule applied.
/// The node rides the same batch as the sync state, so a failed batch leaves
/// neither (MEDULLA-SRC-AUTH-R012.8).
pub fn sync_conflict_mutations(
    connection: &str,
    class: &str,
    rule_literal: &str,
    outcomes: &[SyncOutcome],
    now_ms: u64,
) -> Vec<SyncMutation> {
    outcomes
        .iter()
        .filter(|outcome| matches!(outcome.action, SyncAction::Conflict { .. }))
        .map(|outcome| {
            let mut properties = Map::new();
            properties.insert("type".into(), json!(SYNC_CONFLICT_CLASS));
            properties.insert("syncConnection".into(), json!(connection));
            properties.insert("syncClass".into(), json!(class));
            properties.insert("syncRecordKey".into(), json!(outcome.record_key));
            if let Some(digest) = &outcome.graph_digest {
                properties.insert("graphDigest".into(), json!(digest));
            }
            if let Some(digest) = &outcome.source_digest {
                properties.insert("sourceDigest".into(), json!(digest));
            }
            properties.insert("conflictRuleApplied".into(), json!(rule_literal));
            properties.insert("syncedAtMs".into(), json!(now_ms));
            SyncMutation {
                node_id: keyed_id(
                    "sync-conflict",
                    CONFLICT_DOMAIN,
                    connection,
                    class,
                    &outcome.record_key,
                ),
                properties,
            }
        })
        .collect()
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
