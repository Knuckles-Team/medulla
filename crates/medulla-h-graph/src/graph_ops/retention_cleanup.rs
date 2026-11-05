//! Retention cleanup step (MEDULLA-DURABLE-KERNEL-R700.2).
//!
//! One bounded step over one graph: aged `Document` nodes (label page in id order
//! after the document cursor, matching when `updated_at` is older than the policy
//! age) and aged orphans (sorted node ids after the orphan cursor, matching when
//! the node has no edges, no `owner_ref` metadata key and a `created_at` older
//! than the policy age). A step examines at most `policy.batch()` candidates and
//! every removal of the step travels in ONE `Method::BatchUpdate` of
//! `remove_node` operations, so it commits as one `MutationBatch`; the existing
//! node-removal path releases the blob holders.
//!
//! The hold checks run per candidate before the batch is built and cannot be
//! disabled: a node with a live inbound edge and a node whose metadata sets
//! `legal_hold` to true are never removed.

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_types::retention_policy::RetentionPolicy;
use serde::{Deserialize, Serialize};

/// The label whose aged nodes a step removes.
pub const DOCUMENT_LABEL: &str = "Document";

/// Exclusive id cursors of the two candidate scans.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCursor {
    pub document: Option<String>,
    pub orphan: Option<String>,
}

/// Why a node was removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    AgedDocument,
    Orphan,
}

/// The outcome of one cleanup step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCleanupReport {
    /// Candidates examined (at most the policy batch).
    pub examined: u32,
    /// Ids removed, in examination order, with their reason.
    pub removed: Vec<(String, RemovalReason)>,
    /// Candidates that matched but a hold check kept.
    pub held: u32,
    /// The cursor the step started from.
    pub cursor: RetentionCursor,
    /// The last examined id of each scan; the next step resumes after it.
    pub next_cursor: RetentionCursor,
}

impl RetentionCleanupReport {
    /// The single `BatchUpdate` that removes this step's nodes, `None` when the
    /// step removes nothing.
    pub fn removal_batch(&self) -> Result<Option<Method>, String> {
        if self.removed.is_empty() {
            return Ok(None);
        }
        let operations: Vec<serde_json::Value> = self
            .removed
            .iter()
            .map(|(id, _)| serde_json::json!({"op": "remove_node", "id": id}))
            .collect();
        let operations_msgpack = rmp_serde::to_vec_named(&operations)
            .map_err(|error| format!("retention cleanup batch encode failed: {error}"))?;
        Ok(Some(Method::BatchUpdate { operations_msgpack }))
    }
}

fn row_ms(row: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    row.get(key)
        .or_else(|| row.get("metadata").and_then(|m| m.get(key)))
        .and_then(serde_json::Value::as_u64)
}

fn metadata_has(row: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    row.get("metadata")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|m| m.contains_key(key))
}

fn legal_hold(row: &serde_json::Map<String, serde_json::Value>) -> bool {
    row.get("metadata")
        .and_then(|m| m.get("legal_hold"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn older_than(at: Option<u64>, policy: &RetentionPolicy, now_ms: u64) -> bool {
    at.is_some_and(|at| now_ms.saturating_sub(at) > policy.age_ms())
}

/// Fixed hold checks: a live inbound edge or a legal hold keeps the node. An
/// unknown node (removed concurrently) is held, never removed blind.
fn is_held(core: &GraphCore, id: &str, row: &serde_json::Map<String, serde_json::Value>) -> bool {
    legal_hold(row) || core.in_degree(id).map_or(true, |d| d > 0)
}

fn decode_row(blob: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    medulla_types::msgpack::decode_property_object(blob).unwrap_or_default()
}

/// Run one cleanup step over `core`. Pure: it reads the graph and returns the
/// report; the caller commits [`RetentionCleanupReport::removal_batch`].
pub fn cleanup_graph(
    core: &GraphCore,
    policy: &RetentionPolicy,
    cursor: &RetentionCursor,
    now_ms: u64,
) -> RetentionCleanupReport {
    let budget = policy.batch() as usize;
    let mut report = RetentionCleanupReport {
        cursor: cursor.clone(),
        next_cursor: cursor.clone(),
        ..Default::default()
    };

    let documents =
        core.get_nodes_by_label_page(DOCUMENT_LABEL, cursor.document.as_deref(), budget);
    for (id, blob) in documents {
        report.examined += 1;
        let row = decode_row(&blob);
        if older_than(row_ms(&row, "updated_at"), policy, now_ms) {
            if is_held(core, &id, &row) {
                report.held += 1;
            } else {
                report
                    .removed
                    .push((id.clone(), RemovalReason::AgedDocument));
            }
        }
        report.next_cursor.document = Some(id);
    }

    let remaining = budget.saturating_sub(report.examined as usize);
    if remaining == 0 {
        return report;
    }
    let mut ids: Vec<String> = core
        .node_ids()
        .into_iter()
        .filter(|id| {
            cursor
                .orphan
                .as_deref()
                .is_none_or(|after| id.as_str() > after)
        })
        .collect();
    ids.sort_unstable();
    for id in ids.into_iter().take(remaining) {
        report.examined += 1;
        let already = report.removed.iter().any(|(removed, _)| *removed == id);
        let isolated = core.in_degree(&id) == Ok(0) && core.out_degree(&id) == Ok(0);
        if !already && isolated {
            let row = core
                .get_node_properties(&id)
                .map(|blob| decode_row(&blob))
                .unwrap_or_default();
            if !metadata_has(&row, "owner_ref")
                && older_than(row_ms(&row, "created_at"), policy, now_ms)
            {
                if legal_hold(&row) {
                    report.held += 1;
                } else {
                    report.removed.push((id.clone(), RemovalReason::Orphan));
                }
            }
        }
        report.next_cursor.orphan = Some(id);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::retention_policy::HoldCheck;

    const DAY: u64 = 86_400_000;
    const NOW: u64 = 100 * DAY;

    fn props(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).expect("encode fixture")
    }

    fn fixture(n: usize) -> GraphCore {
        let core = GraphCore::new();
        for i in 0..n {
            let id = format!("n{i:06}");
            let row = if i % 2 == 0 {
                serde_json::json!({"type": "Document", "updated_at": 1})
            } else {
                serde_json::json!({"type": "Note", "created_at": 1})
            };
            core.add_node(id, props(row));
        }
        core
    }

    // spec: MEDULLA-DURABLE-KERNEL-R700.2
    #[test]
    fn step_examines_batch_in_id_order() {
        let policy = RetentionPolicy::new(
            DAY,
            64,
            vec![HoldCheck::LiveReference, HoldCheck::LegalHold],
        )
        .unwrap();
        for n in [10_usize, 1_000, 100_000] {
            let core = fixture(n);
            let mut cursor = RetentionCursor::default();
            let mut seen_docs: Vec<String> = Vec::new();
            let mut seen_orphans: Vec<String> = Vec::new();
            loop {
                let report = cleanup_graph(&core, &policy, &cursor, NOW);
                assert!(report.examined as usize <= policy.batch() as usize);
                assert_eq!(report.cursor, cursor);
                if report.examined == 0 {
                    break;
                }
                for (id, reason) in &report.removed {
                    match reason {
                        RemovalReason::AgedDocument => seen_docs.push(id.clone()),
                        RemovalReason::Orphan => seen_orphans.push(id.clone()),
                    }
                }
                assert!(report.removal_batch().unwrap().is_some());
                cursor = report.next_cursor;
                if n == 100_000 && seen_docs.len() > 2_000 {
                    break;
                }
            }
            let mut sorted = seen_docs.clone();
            sorted.sort();
            assert_eq!(seen_docs, sorted, "documents leave in id order");
            let mut sorted = seen_orphans.clone();
            sorted.sort();
            assert_eq!(seen_orphans, sorted, "orphans leave in id order");
            if n < 100_000 {
                assert!(seen_docs.len() + seen_orphans.len() >= n / 2);
            }
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R700.2
    #[test]
    fn holds_and_fresh_nodes_survive() {
        let core = GraphCore::new();
        core.add_node(
            "a".into(),
            props(serde_json::json!({"type": "Document", "updated_at": 1})),
        );
        core.add_node(
            "b".into(),
            props(serde_json::json!({"type": "Document", "updated_at": 1, "metadata": {"legal_hold": true}})),
        );
        core.add_node(
            "c".into(),
            props(serde_json::json!({"type": "Document", "updated_at": NOW})),
        );
        core.add_node(
            "d".into(),
            props(serde_json::json!({"type": "Document", "updated_at": 1})),
        );
        core.add_node(
            "ref".into(),
            props(serde_json::json!({"type": "Note", "created_at": NOW})),
        );
        core.add_edge(
            "ref".into(),
            "d".into(),
            props(serde_json::json!({"relationship": "CITES"})),
        )
        .unwrap();
        core.add_node(
            "owned".into(),
            props(serde_json::json!({"created_at": 1, "metadata": {"owner_ref": "x"}})),
        );
        core.add_node("orphan".into(), props(serde_json::json!({"created_at": 1})));
        let policy = RetentionPolicy::new(DAY, 100, vec![HoldCheck::LegalHold]).unwrap();
        let report = cleanup_graph(&core, &policy, &RetentionCursor::default(), NOW);
        assert_eq!(
            report.removed,
            vec![
                ("a".to_string(), RemovalReason::AgedDocument),
                ("orphan".to_string(), RemovalReason::Orphan),
            ]
        );
        assert_eq!(report.held, 2);
    }
}
