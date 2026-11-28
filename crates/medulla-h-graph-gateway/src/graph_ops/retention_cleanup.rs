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
use medulla_mutation_types::retention_cleanup::{
    RetentionCleanupRecord, RetentionCleanupReport as WireReport, RetentionCleanupRunRequest,
};
use medulla_mutation_types::retention_policy::{
    RetentionPolicy, RETENTION_HOLD_CHECK_REQUIRED, RETENTION_POLICY_INVALID,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use serde::{Deserialize, Serialize};

/// The actor both entry points attribute removals to (MEDULLA-DURABLE-KERNEL-R700.3).
pub const RETENTION_ACTOR: &str = "engine:retention-cleanup";
/// The environment key holding the [`RetentionPolicy`] as JSON; unset disables cleanup.
pub const POLICY_ENV: &str = "MEDULLA_RETENTION_CLEANUP";
/// The environment key of the sweep interval, in seconds.
pub const INTERVAL_ENV: &str = "MEDULLA_RETENTION_CLEANUP_SECS";
const DEFAULT_INTERVAL_SECS: u64 = 3_600;
const INTERVAL_SECS_RANGE: (u64, u64) = (60, 86_400);

/// Decode a policy spec. A refusal names its code: a policy without a hold check is
/// `RETENTION_HOLD_CHECK_REQUIRED`, any other invalid policy `RETENTION_POLICY_INVALID`
/// (MEDULLA-DURABLE-KERNEL-R700.4).
pub fn policy_from_spec(spec: &str) -> Result<RetentionPolicy, String> {
    serde_json::from_str::<RetentionPolicy>(spec).map_err(|error| {
        let text = error.to_string();
        if text.starts_with(RETENTION_HOLD_CHECK_REQUIRED)
            || text.starts_with(RETENTION_POLICY_INVALID)
        {
            text
        } else {
            format!("{RETENTION_POLICY_INVALID}: {text}")
        }
    })
}

/// The configured policy, or the coded refusal the served method answers: unset is
/// `INVALID_ARGUMENT`, an invalid policy names its code.
pub fn configured_policy_checked() -> Result<RetentionPolicy, String> {
    let spec = std::env::var(POLICY_ENV).unwrap_or_default();
    if spec.trim().is_empty() {
        return Err(format!(
            "INVALID_ARGUMENT: retention cleanup is not configured ({POLICY_ENV} is unset)"
        ));
    }
    policy_from_spec(&spec)
}

/// The configured policy; unset or invalid means no cleanup (an invalid one is logged).
pub fn configured_policy() -> Option<RetentionPolicy> {
    let spec = std::env::var(POLICY_ENV).ok()?;
    if spec.trim().is_empty() {
        return None;
    }
    match policy_from_spec(&spec) {
        Ok(policy) => Some(policy),
        Err(error) => {
            tracing::error!(%error, "retention cleanup policy ignored");
            None
        }
    }
}

/// The configured sweep interval in seconds, clamped to [60, 86400].
pub fn sweep_interval_secs() -> u64 {
    medulla_server_api::state::positive_runtime_limit_from_env(INTERVAL_ENV, DEFAULT_INTERVAL_SECS)
        .clamp(INTERVAL_SECS_RANGE.0, INTERVAL_SECS_RANGE.1)
}

/// The label whose aged nodes a step removes.
pub const DOCUMENT_LABEL: &str = "Document";
/// The label of the audit node each non-dry step writes (MEDULLA-DURABLE-KERNEL-R700.5).
pub const RECORD_LABEL: &str = "RetentionCleanupRecord";
/// The newest records one graph keeps; each step prunes the rest in its own batch.
pub const RECORDS_KEPT: usize = 100;

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
    /// Of `held`, the candidates a live inbound edge kept.
    pub held_live_reference: u32,
    /// Of `held`, the candidates a legal hold kept.
    pub held_legal_hold: u32,
    /// The cursor the step started from.
    pub cursor: RetentionCursor,
    /// The last examined id of each scan; the next step resumes after it.
    pub next_cursor: RetentionCursor,
}

impl RetentionCursor {
    /// The cursor as the opaque string the wire carries; `None` when both scans
    /// are at their start.
    pub fn to_wire(&self) -> Option<String> {
        if self.document.is_none() && self.orphan.is_none() {
            return None;
        }
        serde_json::to_string(self).ok()
    }

    /// Read a wire cursor; an absent one is the start.
    pub fn from_wire(cursor: Option<&str>) -> Result<Self, String> {
        match cursor {
            None => Ok(Self::default()),
            Some(text) => serde_json::from_str(text).map_err(|_| {
                "INVALID_ARGUMENT: the retention cursor is not one this engine issued".to_string()
            }),
        }
    }
}

impl RetentionCleanupReport {
    /// The report as the served method answers it.
    pub fn to_wire(&self) -> WireReport {
        WireReport {
            scanned: u64::from(self.examined),
            removed: self.removed.len() as u64,
            held_live_reference: u64::from(self.held_live_reference),
            held_legal_hold: u64::from(self.held_legal_hold),
            next_cursor: self.next_cursor.to_wire(),
        }
    }

    /// The `remove_node` operations of this step, in examination order.
    pub fn removal_operations(&self) -> Vec<serde_json::Value> {
        self.removed
            .iter()
            .map(|(id, _)| serde_json::json!({"op": "remove_node", "id": id}))
            .collect()
    }

    /// The single `BatchUpdate` that removes this step's nodes, `None` when the
    /// step removes nothing.
    pub fn removal_batch(&self) -> Result<Option<Method>, String> {
        if self.removed.is_empty() {
            return Ok(None);
        }
        Ok(Some(batch_of(&self.removal_operations())?))
    }

    /// How many nodes of each reason this step removed: `(aged_document, orphan)`.
    pub fn removed_by_reason(&self) -> (u64, u64) {
        let aged = self
            .removed
            .iter()
            .filter(|(_, reason)| *reason == RemovalReason::AgedDocument)
            .count() as u64;
        (aged, self.removed.len() as u64 - aged)
    }
}

fn batch_of(operations: &[serde_json::Value]) -> Result<Method, String> {
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("retention cleanup batch encode failed: {error}"))?;
    Ok(Method::BatchUpdate { operations_msgpack })
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

/// Which hold check keeps a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    LegalHold,
    LiveReference,
}

/// Fixed hold checks: a live inbound edge or a legal hold keeps the node. An
/// unknown node (removed concurrently) is held, never removed blind.
fn hold_of(
    core: &GraphCore,
    id: &str,
    row: &serde_json::Map<String, serde_json::Value>,
) -> Option<Hold> {
    if legal_hold(row) {
        return Some(Hold::LegalHold);
    }
    core.in_degree(id)
        .map_or(true, |d| d > 0)
        .then_some(Hold::LiveReference)
}

impl RetentionCleanupReport {
    fn note_hold(&mut self, hold: Hold) {
        self.held += 1;
        match hold {
            Hold::LegalHold => self.held_legal_hold += 1,
            Hold::LiveReference => self.held_live_reference += 1,
        }
    }
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
            match hold_of(core, &id, &row) {
                Some(hold) => report.note_hold(hold),
                None => report
                    .removed
                    .push((id.clone(), RemovalReason::AgedDocument)),
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
            let is_record =
                row.get("type").and_then(serde_json::Value::as_str) == Some(RECORD_LABEL);
            if !is_record
                && !metadata_has(&row, "owner_ref")
                && older_than(row_ms(&row, "created_at"), policy, now_ms)
            {
                if legal_hold(&row) {
                    report.note_hold(Hold::LegalHold);
                } else {
                    report.removed.push((id.clone(), RemovalReason::Orphan));
                }
            }
        }
        report.next_cursor.orphan = Some(id);
    }
    report
}

/// Serve `RetentionCleanupRun` on the gateway: one bounded step over the request
/// graph, committed as one `BatchUpdate` (the removals, the audit record and the
/// pruning of old records) unless `dry_run` asks only for the report.
pub async fn handle_retention_cleanup_run(ctx: &MutationCtx<'_>, method: &Method) -> Response {
    let Method::RetentionCleanupRun { request } = method else {
        return Response::err(
            ctx.req_id,
            "INVALID_ARGUMENT: not a RetentionCleanupRun".to_string(),
        );
    };
    let planned = match plan_step(ctx, request) {
        Ok(planned) => planned,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let PlannedStep {
        wire,
        operations,
        removed_by_reason: (aged, orphan),
    } = planned;
    let Some(operations_msgpack) = operations else {
        return answer(ctx.req_id, wire);
    };
    let write = Method::BatchUpdate {
        operations_msgpack: operations_msgpack.clone(),
    };
    let plan = MutationPlan::for_method(&write);
    let response =
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
            medulla_graph_algos::algorithms::batch_update(core, &operations_msgpack)
                .and_then(|_| payload(wire))
        })
        .await;
    // The counter moves only after the batch committed.
    if response.error.is_none() {
        let metrics = medulla_kernel::metrics::process();
        metrics.retention_cleanup_removed("aged_document", aged);
        metrics.retention_cleanup_removed("orphan", orphan);
    }
    response
}

fn payload(report: WireReport) -> Result<ResultPayload, String> {
    ResultPayload::of::<medulla_protocol_types::result_contract::storage::RetentionCleanupRun>(
        report,
    )
}

fn answer(req_id: u64, report: WireReport) -> Response {
    match payload(report) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The report of one step, the encoded batch to commit (absent for a dry run) and
/// the removal counts by reason.
struct PlannedStep {
    wire: WireReport,
    operations: Option<Vec<u8>>,
    removed_by_reason: (u64, u64),
}

/// The SHA-256 of the policy's canonical JSON, as lowercase hex.
fn policy_digest(policy: &RetentionPolicy) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_string(policy)
        .map_err(|error| format!("retention policy encode failed: {error}"))?;
    Ok(hex::encode(Sha256::digest(json.as_bytes())))
}

/// The audit node of one step and the operations that add it and prune the graph's
/// records to the newest [`RECORDS_KEPT`]. Record ids sort by time, so the oldest
/// sort first.
fn record_operations(
    core: &GraphCore,
    record: &RetentionCleanupRecord,
) -> Result<Vec<serde_json::Value>, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let mut existing: Vec<String> = core
        .get_nodes_by_label(RECORD_LABEL, RECORDS_KEPT * 100)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    existing.sort_unstable();
    let excess = (existing.len() + 1).saturating_sub(RECORDS_KEPT);
    let mut operations: Vec<serde_json::Value> = existing
        .into_iter()
        .take(excess)
        .map(|id| serde_json::json!({"op": "remove_node", "id": id}))
        .collect();
    let mut properties = serde_json::to_value(record)
        .map_err(|error| format!("retention record encode failed: {error}"))?;
    if let Some(object) = properties.as_object_mut() {
        object.insert("type".to_string(), serde_json::json!(RECORD_LABEL));
    }
    let id = format!(
        "retention-cleanup-record:{:020}:{:06}",
        record.at_ms,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    operations.push(serde_json::json!({"op": "add_node", "id": id, "properties": properties}));
    Ok(operations)
}

fn plan_step(
    ctx: &MutationCtx<'_>,
    request: &RetentionCleanupRunRequest,
) -> Result<PlannedStep, String> {
    let policy = configured_policy_checked()?;
    if request.graph != ctx.graph_name {
        return Err(format!(
            "INVALID_ARGUMENT: the request names graph {} but runs on graph {}",
            request.graph, ctx.graph_name
        ));
    }
    let cursor = RetentionCursor::from_wire(request.cursor.as_deref())?;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let report = cleanup_graph(ctx.core, &policy, &cursor, now_ms);
    let wire = report.to_wire();
    let operations = if request.dry_run {
        None
    } else {
        let record = RetentionCleanupRecord {
            graph: request.graph.clone(),
            cursor_start: request.cursor.clone(),
            cursor_end: wire.next_cursor.clone(),
            scanned: wire.scanned,
            removed: wire.removed,
            held_live_reference: wire.held_live_reference,
            held_legal_hold: wire.held_legal_hold,
            policy_digest: policy_digest(&policy)?,
            at_ms: now_ms,
        };
        let mut operations = report.removal_operations();
        operations.extend(record_operations(ctx.core, &record)?);
        let Method::BatchUpdate { operations_msgpack } = batch_of(&operations)? else {
            return Err("INTERNAL: retention cleanup batch is not a BatchUpdate".to_string());
        };
        Some(operations_msgpack)
    };
    Ok(PlannedStep {
        wire,
        operations,
        removed_by_reason: report.removed_by_reason(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_mutation_types::retention_policy::HoldCheck;

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
        assert_eq!(report.held_legal_hold, 1);
        assert_eq!(report.held_live_reference, 1);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R700.4
    #[test]
    fn policy_refusals_name_their_codes() {
        let no_hold = policy_from_spec(r#"{"age_ms":1,"batch":1,"holds":[]}"#).unwrap_err();
        assert!(
            no_hold.starts_with("RETENTION_HOLD_CHECK_REQUIRED"),
            "{no_hold}"
        );
        for bad in [
            r#"{"age_ms":0,"batch":1,"holds":["legal_hold"]}"#,
            r#"{"age_ms":1,"batch":0,"holds":["legal_hold"]}"#,
            r#"{"age_ms":1,"batch":10001,"holds":["legal_hold"]}"#,
            "not json",
        ] {
            let refusal = policy_from_spec(bad).unwrap_err();
            assert!(
                refusal.starts_with("RETENTION_POLICY_INVALID"),
                "{bad}: {refusal}"
            );
        }
        assert!(policy_from_spec(r#"{"age_ms":1,"batch":1,"holds":["legal_hold"]}"#).is_ok());
    }

    // spec: MEDULLA-DURABLE-KERNEL-R700.5
    #[test]
    fn record_operations_add_one_node_and_prune_to_the_newest() {
        let core = GraphCore::new();
        for i in 0..RECORDS_KEPT {
            core.add_node(
                format!("retention-cleanup-record:{i:020}:000000"),
                props(serde_json::json!({"type": RECORD_LABEL})),
            );
        }
        let record = RetentionCleanupRecord {
            graph: "g".to_string(),
            cursor_start: None,
            cursor_end: Some("{}".to_string()),
            scanned: 3,
            removed: 2,
            held_live_reference: 1,
            held_legal_hold: 0,
            policy_digest: "ab".repeat(32),
            at_ms: NOW,
        };
        let operations = record_operations(&core, &record).unwrap();
        // The oldest record is pruned to make room; the new record is added last.
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0]["op"], "remove_node");
        assert_eq!(
            operations[0]["id"],
            format!("retention-cleanup-record:{:020}:000000", 0)
        );
        assert_eq!(operations[1]["op"], "add_node");
        assert_eq!(operations[1]["properties"]["type"], RECORD_LABEL);
        assert_eq!(operations[1]["properties"]["removed"], 2);
        assert_eq!(
            operations[1]["properties"]["policy_digest"],
            "ab".repeat(32)
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R700.5
    #[test]
    fn record_nodes_are_never_orphan_candidates() {
        let core = GraphCore::new();
        core.add_node(
            "retention-cleanup-record:00000000000000000001:000000".into(),
            props(serde_json::json!({"type": RECORD_LABEL, "created_at": 1})),
        );
        let policy = RetentionPolicy::new(DAY, 100, vec![HoldCheck::LegalHold]).unwrap();
        let report = cleanup_graph(&core, &policy, &RetentionCursor::default(), NOW);
        assert!(report.removed.is_empty());
    }
}
