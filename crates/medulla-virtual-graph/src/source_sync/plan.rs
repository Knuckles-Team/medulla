//! One whole sync as a pure function (MEDULLA-SRC-AUTH-R012.6): the source
//! records, the visible graph nodes and the stored sync state go in; the
//! decisions, the one mutation list and the report come out. The served handler
//! reads the inputs and commits the list; nothing here touches a store.

use std::collections::{BTreeMap, BTreeSet};

use medulla_source_types::source_sync::{
    OutboundDraft, SyncConflictEntry, SyncConflictReason, SyncReport, SyncSkippedType,
};
use medulla_types::contract::Digest256;
use serde_json::{Map, Value};

use super::{
    reconcile, sync_batch, sync_conflict_mutations, sync_state_mutations, ConflictRule, SyncAction,
    RecordScreen, SyncAdapter, SyncMutation, SyncOutcome, SYNCED_FROM_PROPERTY, SYNC_STATE_CLASS,
};

/// Every visible graph node: id to its decoded property map.
pub type GraphNodes = BTreeMap<String, Map<String, Value>>;

/// The result of planning one sync.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPlan {
    /// One decision per record key seen on either side, in key order.
    pub outcomes: Vec<SyncOutcome>,
    /// The inbound changes and the sync state, as the one list a batch commits.
    pub batch: Vec<SyncMutation>,
    /// The counts, the conflicts and the outbound drafts.
    pub report: SyncReport,
}

/// Base digests by record key, read from the `vg:SyncState` nodes of one
/// connection and class.
pub fn base_digests(nodes: &GraphNodes, connection: &str, class: &str) -> BTreeMap<String, String> {
    nodes
        .values()
        .filter(|props| props.get("type").and_then(Value::as_str) == Some(SYNC_STATE_CLASS))
        .filter(|props| props.get("syncConnection").and_then(Value::as_str) == Some(connection))
        .filter(|props| props.get("syncClass").and_then(Value::as_str) == Some(class))
        .filter_map(|props| {
            Some((
                props.get("syncRecordKey")?.as_str()?.to_string(),
                props.get("baseDigest")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Plan one sync of `class` with `connection` under `rule`.
///
/// `source_records` are the records the host read from the pack source; the
/// adapter keys and digests them. The graph side comes from the adapter's
/// `graph_records` over `nodes`. A source record with no key, or two records
/// with one key, refuse the whole sync.
#[allow(clippy::too_many_arguments)]
pub fn plan_sync(
    adapter: &dyn SyncAdapter,
    connection: &str,
    class: &str,
    rule: ConflictRule,
    rule_literal: &str,
    source_records: &[Value],
    nodes: &GraphNodes,
    now_ms: u64,
) -> Result<SyncPlan, String> {
    // Every record is keyed and screened once. The adapter's screen leaves out
    // records of another class and names each source type no catalog class carries
    // (MEDULLA-TYPED-PACKS-R110.6).
    let mut screened: BTreeMap<String, (&Value, RecordScreen)> = BTreeMap::new();
    for record in source_records {
        let key = adapter
            .record_key(record)
            .ok_or_else(|| "INVALID_ARGUMENT: a source record has no key".to_string())?;
        let screen = adapter.screen(record);
        if screened.insert(key.clone(), (record, screen)).is_some() {
            return Err(format!(
                "INVALID_ARGUMENT: the source holds two records with key {key}"
            ));
        }
    }
    let mut source: BTreeMap<String, (&Value, String)> = BTreeMap::new();
    let mut skipped: BTreeMap<String, SyncSkippedType> = BTreeMap::new();
    let mut by_type: BTreeMap<String, u64> = BTreeMap::new();
    for (key, (record, screen)) in &screened {
        for type_name in &screen.unmapped_types {
            skipped
                .entry(type_name.clone())
                .and_modify(|entry| entry.count += 1)
                .or_insert_with(|| SyncSkippedType {
                    type_name: type_name.clone(),
                    first_key: key.clone(),
                    count: 1,
                });
        }
        if !screen.accepted {
            continue;
        }
        if let Some(type_name) = &screen.type_name {
            *by_type.entry(type_name.clone()).or_insert(0) += 1;
        }
        source.insert(key.clone(), (*record, adapter.record_digest(record)));
    }
    // The `syncedFrom` stamp is evidence, not record content: it is removed from every
    // node before the adapter builds the graph side, so a re-sync digests the same
    // record (MEDULLA-TYPED-PACKS-R110.7).
    let unstamped: GraphNodes = nodes
        .iter()
        .map(|(id, props)| {
            let mut props = props.clone();
            props.remove(SYNCED_FROM_PROPERTY);
            (id.clone(), props)
        })
        .collect();
    let graph = adapter.graph_records(&unstamped);
    let graph_digests: BTreeMap<&String, String> = graph
        .iter()
        .map(|(key, record)| (key, adapter.record_digest(record)))
        .collect();
    let bases = base_digests(nodes, connection, class);

    let keys: BTreeSet<&String> = source.keys().chain(graph.keys()).collect();
    let mut outcomes = Vec::new();
    let mut inbound = Vec::new();
    let mut report = SyncReport {
        connection_iri: connection.to_string(),
        class_iri: class.to_string(),
        unchanged: 0,
        applied: 0,
        proposed: 0,
        conflicts: Vec::new(),
        outbound: Vec::new(),
        queued: Vec::new(),
        queue_id: None,
        skipped: skipped.into_values().collect(),
        by_type,
    };
    for key in keys {
        let base = bases.get(key).map(String::as_str);
        let graph_digest = graph_digests.get(key).map(String::as_str);
        let source_digest = source.get(key).map(|(_, digest)| digest.as_str());
        let action = reconcile(base, graph_digest, source_digest, rule);
        match &action {
            SyncAction::Unchanged => report.unchanged += 1,
            SyncAction::ApplyInbound => {
                report.applied += 1;
                if let Some((record, _)) = source.get(key) {
                    for mut mutation in adapter.inbound_mutations(key, record) {
                        mutation.properties.insert(
                            SYNCED_FROM_PROPERTY.to_string(),
                            Value::String(connection.to_string()),
                        );
                        inbound.push(mutation);
                    }
                }
            }
            SyncAction::ProposeOutbound | SyncAction::Conflict { .. } => {}
        }
        // A graph-only change over a source store that moved on is still proposed;
        // the report names it so the approver sees the version it would replace.
        let source_ahead = matches!(action, SyncAction::ProposeOutbound)
            && match (graph.get(key), source.get(key)) {
                (Some(graph_record), Some((source_record, _))) => {
                    adapter.version_ahead(graph_record, source_record)
                }
                _ => false,
            };
        let conflict_reason = match &action {
            SyncAction::Conflict { .. } => Some(SyncConflictReason::BothChanged),
            _ if source_ahead => Some(SyncConflictReason::SourceVersionAhead),
            _ => None,
        };
        if let Some(reason) = conflict_reason {
            report.conflicts.push(SyncConflictEntry {
                record_key: key.clone(),
                graph_digest: graph_digest.map(str::to_string),
                source_digest: source_digest.map(str::to_string),
                rule: rule_literal.to_string(),
                reason,
            });
        }
        if action.proposes_outbound() {
            // A graph record that was deleted has no upsert payload to propose.
            if let Some(record) = graph.get(key) {
                let payload = adapter.outbound_payload(key, record);
                report.outbound.push(OutboundDraft {
                    record_key: key.clone(),
                    payload_digest: Digest256::sha256(payload.to_string().as_bytes()).to_hex(),
                });
                report.proposed += 1;
            }
        }
        outcomes.push(SyncOutcome {
            record_key: key.clone(),
            action,
            base_digest: base.map(str::to_string),
            graph_digest: graph_digest.map(str::to_string),
            source_digest: source_digest.map(str::to_string),
            source_revision: source
                .get(key)
                .and_then(|(record, _)| record.get("revision"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    let mut state = sync_state_mutations(connection, class, &outcomes, now_ms);
    state.extend(sync_conflict_mutations(
        connection,
        class,
        rule_literal,
        &outcomes,
        now_ms,
    ));
    Ok(SyncPlan {
        outcomes,
        batch: sync_batch(inbound, state),
        report,
    })
}
