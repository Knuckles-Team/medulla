//! WorkItem commit.rs transitions.

use super::*;

/// The lease fence a `CommitWorkItemResult` must satisfy: the caller owns the
/// lease, the item is live, and the lease has not expired.
pub fn commit_work_item_lease_is_valid(
    props: &serde_json::Map<String, serde_json::Value>,
    worker_id: &str,
    lease_epoch: u64,
    fencing_token: u64,
    now_ms: u64,
) -> bool {
    property_string(props, "lease_owner") == worker_id
        && super::machine::holds_lease_row(props)
        && property_u64(props, "lease_epoch") == lease_epoch
        && property_u64(props, "fencing_token") == fencing_token
        && property_f64(props, "lease_expires_at") >= now_ms as f64 / 1000.0
}

/// The three short-circuit responses of a commit, in their original order:
/// a tenant mismatch reads as `missing`, an already-terminal item as `noop`,
/// and a failed lease fence as `fenced`.  `Ok(None)` means the commit proceeds.
pub fn commit_work_item_result_precheck(
    props: &serde_json::Map<String, serde_json::Value>,
    work_item_id: &str,
    tenant: &str,
    worker_id: &str,
    lease_epoch: u64,
    fencing_token: u64,
    now_ms: u64,
) -> Option<
    medulla_protocol_types::result_contract::coordination::WorkItemTransition<
        medulla_protocol_types::result_contract::coordination::WorkItemCommitStatus,
    >,
> {
    if property_string(props, "tenant") != tenant {
        return Some(unchanged_transition(
            medulla_protocol_types::result_contract::coordination::WorkItemCommitStatus::Missing,
            None,
        ));
    }
    if super::machine::is_terminal_row(props) {
        return Some(unchanged_transition(
            medulla_protocol_types::result_contract::coordination::WorkItemCommitStatus::Noop,
            Some(work_item_id),
        ));
    }
    if !commit_work_item_lease_is_valid(props, worker_id, lease_epoch, fencing_token, now_ms) {
        return Some(unchanged_transition(
            medulla_protocol_types::result_contract::coordination::WorkItemCommitStatus::Fenced,
            Some(work_item_id),
        ));
    }
    None
}

/// Fire the commit edge (W05 to W14) and report the committed status. A retryable
/// failure with attempts left goes back to `ready` with backoff and a bumped fence and
/// reports `retry_scheduled`; otherwise the item goes terminal (`dead_letter` for a
/// retryable failure whose attempts are spent, else the outcome itself). The caller has
/// validated the fence and the live lease.
pub fn commit_work_item_apply_status(
    props: &mut serde_json::Map<String, serde_json::Value>,
    outcome: &str,
    retryable: bool,
    lease_epoch: u64,
    fencing_token: u64,
    now_s: f64,
) -> Result<&'static str, String> {
    use medulla_work_types::work_item_machine::{CommitOutcome, WorkItemEvent, WorkItemFacts};
    let commit = CommitOutcome::from_commit(outcome, retryable).ok_or_else(|| {
        "CommitWorkItemResult outcome must be succeeded, failed, or cancelled".to_string()
    })?;
    let attempts = property_u64(props, "attempt");
    let facts = WorkItemFacts {
        fence_matches: true,
        ..super::machine::row_facts(props, now_s, true)
    };
    let next = super::machine::fire(props, WorkItemEvent::Commit(commit), &facts)?;
    if !next.is_terminal() {
        let backoff = property_f64(props, "backoff_base_s").max(1.0)
            * 2f64.powi(attempts.saturating_sub(1).min(31) as i32);
        props.insert(
            "next_retry_at".into(),
            serde_json::Value::from(now_s + backoff),
        );
        props.insert(
            "lease_epoch".into(),
            serde_json::Value::from((lease_epoch).saturating_add(1)),
        );
        props.insert(
            "fencing_token".into(),
            serde_json::Value::from((fencing_token).saturating_add(1)),
        );
        return Ok("retry_scheduled");
    }
    props.insert("completed_at".into(), serde_json::Value::from(now_s));
    Ok(next.as_stored())
}

/// Record the commit's lease/result bookkeeping on the item.
pub fn commit_work_item_record_result_refs(
    props: &mut serde_json::Map<String, serde_json::Value>,
    worker_id: &str,
    result_ref: &Option<String>,
    error_ref: &Option<String>,
    now_s: f64,
) {
    props.insert(
        "result_ref".into(),
        result_ref
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    props.insert(
        "error_ref".into(),
        error_ref
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    props.insert("lease_owner".into(), serde_json::Value::Null);
    props.insert(
        "last_lease_owner".into(),
        serde_json::Value::String(worker_id.to_string()),
    );
    props.insert("lease_expires_at".into(), serde_json::Value::Null);
    props.insert("updated_at".into(), serde_json::Value::from(now_s));
}

/// Decrement each downstream child's dependency count after a successful
/// commit, releasing a child to `ready` once its last dependency clears.
/// Children that no longer exist are skipped, as before.
pub fn commit_work_item_release_downstream(
    graph: &str,
    props: &serde_json::Map<String, serde_json::Value>,
    now_s: f64,
    nodes: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
    crypto: DurableCrypto<'_>,
    changed: &mut Vec<String>,
) -> Result<(), String> {
    let downstream = props
        .get("downstream_ids")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for child in downstream.iter().filter_map(serde_json::Value::as_str) {
        let child_bytes = nodes
            .get((graph, child))?
            .map(|value| crypto.unseal(value.value()))
            .transpose()?;
        let Some(child_bytes) = child_bytes else {
            continue;
        };
        let mut child_props: serde_json::Map<String, serde_json::Value> =
            decode_durable(&child_bytes)?;
        let count = property_u64(&child_props, "dep_count").saturating_sub(1);
        child_props.insert("dep_count".into(), serde_json::Value::from(count));
        // W01: the last open dependency cleared.
        if count == 0
            && super::machine::stored_status(&child_props)?
                == medulla_work_types::work_item_machine::WorkItemStatus::Submitted
        {
            let facts = super::machine::row_facts(&child_props, now_s, false);
            super::machine::fire(
                &mut child_props,
                medulla_work_types::work_item_machine::WorkItemEvent::DepsSatisfied,
                &facts,
            )?;
        }
        child_props.insert("updated_at".into(), serde_json::Value::from(now_s));
        write_work_item_props(nodes, graph, child, &mut child_props, crypto)?;
        changed.push(child.to_string());
    }
    Ok(())
}

pub struct CommitWorkItemResultInput<'args, 'table, 'crypto> {
    pub graph: &'args str,
    pub tenant: &'args str,
    pub work_item_id: &'args str,
    pub worker_id: &'args str,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub outcome: &'args str,
    pub result_ref: &'args Option<String>,
    pub error_ref: &'args Option<String>,
    pub retryable: bool,
    pub now_ms: u64,
    pub outcome_extension:
        Option<&'args medulla_result_types::outcome_bundle::TerminalOutcomeExtension>,
    pub run_completion: Option<&'args medulla_work_types::run_completion::RunCompletionInput>,
    pub batch_id: &'args str,
    pub nodes: &'args mut ScopedOwnerTableMut<'table, (&'static str, &'static str), &'static [u8]>,
    /// The edge rows the terminal commit links its run trace through (R133.8).
    pub edges: &'args mut NativeEdgeRows<'table>,
    pub holds: &'args mut ScopedOwnerTableMut<'table, (&'static str, &'static str), &'static [u8]>,
    pub work_item_index:
        &'args ScopedOwnerTableMut<'table, (&'static str, &'static str, u64), &'static str>,
    pub counters:
        &'args mut ScopedOwnerTableMut<'table, (&'static str, &'static str), &'static [u8]>,
    pub pressure_index: &'args mut ScopedOwnerTableMut<
        'table,
        (
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            u64,
            &'static str,
        ),
        u8,
    >,
    pub policies: &'args ScopedOwnerTableMut<'table, (&'static str, &'static str), &'static [u8]>,
    pub crypto: DurableCrypto<'crypto>,
}

pub fn apply_commit_work_item_result_row(
    input: CommitWorkItemResultInput<'_, '_, '_>,
) -> Result<Option<medulla_protocol_types::protocol::ResultPayload>, String> {
    let current = input
        .nodes
        .get((input.graph, input.work_item_id))?
        .map(|value| input.crypto.unseal(value.value()))
        .transpose()?;
    let Some(bytes) = current else {
        return commit_transition_result(unchanged_transition(
            medulla_protocol_types::result_contract::coordination::WorkItemCommitStatus::Missing,
            None,
        ))
        .map(Some);
    };
    let mut props: serde_json::Map<String, serde_json::Value> = decode_durable(&bytes)?;
    let pre_props = props.clone();
    validate_ordinary_completion(&input, &props)?;
    if let Some(refusal) = commit_work_item_result_precheck(
        &props,
        input.work_item_id,
        input.tenant,
        input.worker_id,
        input.lease_epoch,
        input.fencing_token,
        input.now_ms,
    ) {
        return commit_transition_result(refusal).map(Some);
    }
    if !matches!(input.outcome, "succeeded" | "failed" | "cancelled") {
        return Err("CommitWorkItemResult outcome must be succeeded, failed, or cancelled".into());
    }
    if let Some(extension) = input.outcome_extension {
        validate_terminal_extension_binding(
            &props,
            extension,
            TerminalCommitClaim {
                worker_id: input.worker_id,
                work_item_id: input.work_item_id,
                fencing_token: input.fencing_token,
                outcome: input.outcome,
                result_ref: input.result_ref,
                batch_id: input.batch_id,
            },
        )?;
        validate_run_trace_receipt(extension)?;
        validate_recursive_trace_receipts(extension)?;
        ensure_receipt_rows_absent(input.graph, &extension.receipt_nodes, input.nodes)?;
    }
    let now_s = input.now_ms as f64 / 1000.0;
    let committed_status = commit_work_item_apply_status(
        &mut props,
        input.outcome,
        input.retryable,
        input.lease_epoch,
        input.fencing_token,
        now_s,
    )?;
    commit_work_item_record_result_refs(
        &mut props,
        input.worker_id,
        input.result_ref,
        input.error_ref,
        now_s,
    );
    development_lane::transition_work_item_terminal_hold(
        input.graph,
        &pre_props,
        input.work_item_id,
        committed_status,
        false,
        property_u64(&props, "attempt"),
        property_u64(&props, "lease_epoch"),
        property_u64(&props, "fencing_token"),
        property_string(&props, "work_item_fence"),
        input.holds,
        input.work_item_index,
        input.counters,
        input.pressure_index,
        input.policies,
        input.crypto,
    )?;
    record_outcome_refs(&mut props, committed_status, input.outcome_extension)?;
    if let Some(completion) = input.run_completion {
        use sha2::{Digest, Sha256};
        let payload = medulla_work_types::run_completion::RunCompletionPayload {
            completion: completion.clone(),
            tenant: input.tenant.into(),
            work_item_id: input.work_item_id.into(),
            batch_id: input.batch_id.into(),
            outcome: committed_status.into(),
            result_ref: input.result_ref.clone(),
            error_ref: input.error_ref.clone(),
        };
        let bytes = rmp_serde::to_vec_named(&payload).map_err(|e| e.to_string())?;
        props.insert(
            medulla_work_types::run_completion::RUN_COMPLETION_BATCH.into(),
            input.batch_id.into(),
        );
        props.insert(
            medulla_work_types::run_completion::RUN_COMPLETION_DIGEST.into(),
            hex::encode(Sha256::digest(&bytes)).into(),
        );
    }
    write_work_item_props(
        input.nodes,
        input.graph,
        input.work_item_id,
        &mut props,
        input.crypto,
    )?;

    let mut changed = vec![input.work_item_id.to_string()];
    if committed_status == "succeeded" {
        commit_work_item_release_downstream(
            input.graph,
            &props,
            now_s,
            input.nodes,
            input.crypto,
            &mut changed,
        )?;
    }
    if committed_status != "retry_scheduled" {
        if let Some(extension) = input.outcome_extension {
            apply_receipt_rows(
                ReceiptRowsInput {
                    graph: input.graph,
                    tenant: input.tenant,
                    work_item_id: input.work_item_id,
                    lease_epoch: input.lease_epoch,
                    committed_status,
                    receipt_nodes: &extension.receipt_nodes,
                },
                input.nodes,
                input.edges,
                input.crypto,
            )?;
        }
    }
    commit_transition_result(
        medulla_protocol_types::result_contract::coordination::WorkItemTransition {
            status: declared_commit_status(committed_status)?,
            work_item_id: Some(input.work_item_id.to_string()),
            lease_epoch: Some(input.lease_epoch),
            fencing_token: Some(input.fencing_token),
            changed_work_item_ids: changed,
        },
    )
    .map(Some)
}

/// The terminal commit an outcome bundle has to be bound to.
///
/// These are the admitted `Method::CommitWorkItemResult` facts plus the batch
/// the commit rides in. A bundle is only trustworthy if it agrees with ALL of
/// them at once -- a bundle that names the right work item but the wrong worker,
/// fence or batch is exactly the forgery this check exists to refuse -- so they
/// are carried as one claim rather than six positional facts a caller can
/// transpose.
struct TerminalCommitClaim<'a> {
    /// The worker the lease is held by, and which the bundle's
    /// `executor_lease_actor` must name.
    pub worker_id: &'a str,
    pub work_item_id: &'a str,
    pub fencing_token: u64,
    /// `succeeded` / `failed` / `cancelled`, already validated by the caller.
    pub outcome: &'a str,
    pub result_ref: &'a Option<String>,
    /// The mutation batch this commit rides in; the bundle's outbox id must
    /// match it, so a bundle cannot be replayed under a different batch.
    pub batch_id: &'a str,
}

fn validate_terminal_extension_binding(
    props: &serde_json::Map<String, serde_json::Value>,
    extension: &medulla_result_types::outcome_bundle::TerminalOutcomeExtension,
    claim: TerminalCommitClaim<'_>,
) -> Result<(), String> {
    let TerminalCommitClaim {
        worker_id,
        work_item_id,
        fencing_token,
        outcome,
        result_ref,
        batch_id,
    } = claim;
    extension.validate()?;
    let bundle = &extension.outcome_bundle;
    if bundle.outcome != outcome {
        return Err(
            "terminal outcome bundle outcome does not match the terminal method".to_string(),
        );
    }
    if bundle.work_item_id != work_item_id
        || bundle.fence_token != fencing_token
        || bundle.result_ref != *result_ref
    {
        return Err("terminal outcome bundle does not match the WorkItem CAS".to_string());
    }
    if bundle.executor_lease_actor != worker_id {
        return Err("terminal outcome bundle executor does not own the lease".to_string());
    }
    if bundle.outbox_id != batch_id {
        return Err("terminal outcome bundle outbox does not match the mutation batch".to_string());
    }
    let metadata = props
        .get("metadata")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "WorkItem metadata is missing delegation bindings".to_string())?;
    let context = props
        .get("context")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "WorkItem context is missing delegation bindings".to_string())?;
    require_binding(
        "delegation_id",
        metadata.get("delegation_id"),
        &bundle.delegation_id,
    )?;
    require_binding("run_id", metadata.get("run_id"), &bundle.run_id)?;
    require_binding(
        "delegator_id",
        context.get("agent_id"),
        &bundle.delegator_id,
    )?;
    require_binding(
        "selected_agent_id",
        metadata.get("agent_id"),
        &bundle.selected_agent_id,
    )?;
    require_digest_binding(
        "capability_digest",
        metadata.get("capability_digest"),
        &bundle.capability_digest,
    )?;
    for (field, expected) in [
        ("catalog_digest", &bundle.catalog_digest),
        ("model_digest", &bundle.model_digest),
    ] {
        require_digest_binding(field, props.get(field), expected)?;
    }
    require_digest_binding(
        "policy_digest",
        props.get("policy_digest"),
        &bundle.policy_digest,
    )
}

fn require_binding(
    field: &str,
    actual: Option<&serde_json::Value>,
    expected: &str,
) -> Result<(), String> {
    let actual = actual
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("WorkItem binding '{field}' is missing"))?;
    if actual != expected {
        return Err(format!(
            "WorkItem binding '{field}' does not match terminal outcome"
        ));
    }
    Ok(())
}

fn require_digest_binding(
    field: &str,
    actual: Option<&serde_json::Value>,
    expected: &str,
) -> Result<(), String> {
    let actual = actual
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("WorkItem binding '{field}' is missing"))?;
    let normalized = actual.strip_prefix("sha256:").unwrap_or(actual);
    if normalized != expected {
        return Err(format!(
            "WorkItem binding '{field}' does not match terminal outcome"
        ));
    }
    Ok(())
}

fn ensure_receipt_rows_absent(
    graph: &str,
    receipt_nodes: &[medulla_result_types::outcome_bundle::ReceiptNode],
    nodes: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
) -> Result<(), String> {
    for receipt in receipt_nodes {
        if nodes.get((graph, receipt.node_id.as_str()))?.is_some() {
            return Err(format!(
                "terminal receipt node '{}' already exists",
                receipt.node_id
            ));
        }
    }
    Ok(())
}

/// Bind a terminal commit's provenance to the WorkItem row (graph-os MEDULLA-3), so
/// `GetWorkItemOutcome` resolves it from the row rather than from a node id a
/// caller names, and verifies the OutcomeEvaluation receipt against the digest
/// recorded here. Skipped on a scheduled retry: receipts are written only with
/// a real terminal transition.
fn record_outcome_refs(
    props: &mut serde_json::Map<String, serde_json::Value>,
    committed_status: &str,
    extension: Option<&medulla_result_types::outcome_bundle::TerminalOutcomeExtension>,
) -> Result<(), String> {
    use medulla_work_types::{
        work_item_read::WORK_ITEM_OUTCOME_DIGEST, work_item_read::WORK_ITEM_OUTCOME_REF,
        work_item_read::WORK_ITEM_TOOL_CALL_REFS, work_item_read::WORK_ITEM_TRACE_REF,
    };
    let Some(extension) = extension.filter(|_| committed_status != "retry_scheduled") else {
        return Ok(());
    };
    use medulla_work_types::{
        work_item_read::WORK_ITEM_EVENT_BATCH, work_item_read::WORK_ITEM_EVENT_DIGEST,
    };
    use sha2::{Digest, Sha256};
    let bundle = &extension.outcome_bundle;
    let event_bytes = rmp_serde::to_vec_named(&extension.run_event)
        .map_err(|error| format!("terminal event encoding failed: {error}"))?;
    props.insert(
        WORK_ITEM_EVENT_BATCH.into(),
        bundle.outbox_id.clone().into(),
    );
    props.insert(
        WORK_ITEM_EVENT_DIGEST.into(),
        hex::encode(Sha256::digest(&event_bytes)).into(),
    );
    let outcome_digest = extension
        .receipt_nodes
        .iter()
        .find(|receipt| receipt.node_id == bundle.outcome_ref)
        .map(|receipt| receipt.payload_digest.clone());
    props.insert(
        WORK_ITEM_OUTCOME_REF.into(),
        bundle.outcome_ref.clone().into(),
    );
    props.insert(WORK_ITEM_OUTCOME_DIGEST.into(), outcome_digest.into());
    props.insert(WORK_ITEM_TRACE_REF.into(), bundle.trace_ref.clone().into());
    props.insert(
        WORK_ITEM_TOOL_CALL_REFS.into(),
        serde_json::json!(bundle.tool_call_refs),
    );
    Ok(())
}

/// The facts `apply_receipt_rows` writes a terminal commit's receipt rows from.
struct ReceiptRowsInput<'a> {
    graph: &'a str,
    tenant: &'a str,
    work_item_id: &'a str,
    lease_epoch: u64,
    committed_status: &'a str,
    receipt_nodes: &'a [medulla_result_types::outcome_bundle::ReceiptNode],
}

/// The property of a `RunTrace` receipt node that carries its [`RunTraceRecord`]
/// beside the identity bindings every receipt node holds (R133.8).
const RUN_TRACE_FIELD: &str = "run_trace";

fn run_trace_invalid(detail: impl std::fmt::Display) -> String {
    format!(
        "{}: {detail}",
        medulla_types::contract::ServerErrorCode::RunTraceInvalid.as_str()
    )
}

/// Decode every `RunTrace` receipt node of a terminal commit and refuse
/// `RUN_TRACE_INVALID` when its record does not decode or names another run
/// than the outcome bundle (MEDULLA-DECISION-ENGINE-R133.8). Runs before any
/// write, so a refusal changes no state.
fn validate_run_trace_receipt(
    extension: &medulla_result_types::outcome_bundle::TerminalOutcomeExtension,
) -> Result<(), String> {
    use medulla_result_types::outcome_bundle::ReceiptNodeKind;
    for receipt in extension
        .receipt_nodes
        .iter()
        .filter(|node| node.kind == ReceiptNodeKind::RunTrace)
    {
        let properties: std::collections::BTreeMap<String, serde_json::Value> =
            rmp_serde::from_slice(&receipt.properties_msgpack).map_err(|error| {
                run_trace_invalid(format!(
                    "receipt node '{}' properties do not decode: {error}",
                    receipt.node_id
                ))
            })?;
        let raw = properties.get(RUN_TRACE_FIELD).ok_or_else(|| {
            run_trace_invalid(format!(
                "receipt node '{}' carries no {RUN_TRACE_FIELD} record",
                receipt.node_id
            ))
        })?;
        let record: medulla_decision_types::decision::run_governance::RunTraceRecord =
            serde_json::from_value(raw.clone()).map_err(|error| {
                run_trace_invalid(format!(
                    "receipt node '{}' record does not decode: {error}",
                    receipt.node_id
                ))
            })?;
        if record.run_id != extension.outcome_bundle.run_id {
            return Err(run_trace_invalid(format!(
                "receipt node '{}' names run '{}' but the bundle names '{}'",
                receipt.node_id, record.run_id, extension.outcome_bundle.run_id
            )));
        }
    }
    Ok(())
}

/// The property of a `RecursiveTraceNode` receipt node that carries its
/// [`RecursiveTraceNode`] record (R133.7.2).
const RECURSIVE_TRACE_FIELD: &str = "recursive_trace";

/// Decode the recursive trace nodes of the receipt set, then check them as one
/// snapshot. A record that does not decode or names another run refuses
/// `RUN_TRACE_INVALID`; a snapshot failure refuses with its own code. Runs
/// before any write (MEDULLA-DECISION-ENGINE-R133.7.2).
fn validate_recursive_trace_receipts(
    extension: &medulla_result_types::outcome_bundle::TerminalOutcomeExtension,
) -> Result<(), String> {
    let nodes =
        decode_recursive_trace_nodes(&extension.receipt_nodes, &extension.outcome_bundle.run_id)?;
    medulla_decision_types::decision::recursive_trace::validate_snapshot(&nodes)
        .map_err(|refusal| format!("{}: {}", refusal.code(), refusal))
}

fn decode_recursive_trace_nodes(
    receipts: &[medulla_result_types::outcome_bundle::ReceiptNode],
    run_id: &str,
) -> Result<Vec<medulla_decision_types::decision::recursive_trace::RecursiveTraceNode>, String> {
    use medulla_result_types::outcome_bundle::ReceiptNodeKind;
    let mut nodes = Vec::new();
    for receipt in receipts
        .iter()
        .filter(|node| node.kind == ReceiptNodeKind::RecursiveTraceNode)
    {
        let properties: std::collections::BTreeMap<String, serde_json::Value> =
            rmp_serde::from_slice(&receipt.properties_msgpack).map_err(|error| {
                run_trace_invalid(format!(
                    "receipt node '{}' properties do not decode: {error}",
                    receipt.node_id
                ))
            })?;
        let raw = properties.get(RECURSIVE_TRACE_FIELD).ok_or_else(|| {
            run_trace_invalid(format!(
                "receipt node '{}' carries no {RECURSIVE_TRACE_FIELD} record",
                receipt.node_id
            ))
        })?;
        let node: medulla_decision_types::decision::recursive_trace::RecursiveTraceNode =
            serde_json::from_value(raw.clone()).map_err(|error| {
                run_trace_invalid(format!(
                    "receipt node '{}' record does not decode: {error}",
                    receipt.node_id
                ))
            })?;
        if node.run_id != run_id || node.node_id != receipt.node_id {
            return Err(run_trace_invalid(format!(
                "receipt node '{}' names run '{}' node '{}' but the commit names run '{run_id}'",
                receipt.node_id, node.run_id, node.node_id
            )));
        }
        nodes.push(node);
    }
    Ok(nodes)
}

/// Write the receipt nodes of a terminal commit, and link a `RunTrace` node from
/// the work item's execution unit with the `produced` edge that
/// `medulla_durable::project` returns for the unit's mirror (R133.8). `project`
/// is pure; the edge row is written here, in the commit's own transaction. The
/// unit's graph node is the work item row itself, so the edge starts there.
fn apply_receipt_rows(
    input: ReceiptRowsInput<'_>,
    nodes: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
    edges: &mut NativeEdgeRows<'_>,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    use medulla_result_types::outcome_bundle::ReceiptNodeKind;
    let graph = input.graph;
    for receipt in input.receipt_nodes {
        let stored = match receipt.kind {
            ReceiptNodeKind::RunTrace => {
                stamp_class(&receipt.properties_msgpack, input.tenant, "RunTrace")?
            }
            ReceiptNodeKind::RecursiveTraceNode => stamp_class(
                &receipt.properties_msgpack,
                input.tenant,
                "RecursiveTraceNode",
            )?,
            _ => receipt.properties_msgpack.clone(),
        };
        let sealed = crypto.seal(&stored);
        nodes
            .insert((graph, receipt.node_id.as_str()), sealed.as_ref())
            .map_err(|error| error.to_string())?;
        if receipt.kind == ReceiptNodeKind::RecursiveTraceNode {
            write_child_of_edge(&input, receipt, edges, crypto)?;
            continue;
        }
        if receipt.kind != ReceiptNodeKind::RunTrace {
            continue;
        }
        let mirror = medulla_durable::DurableExecutionUnitMirror {
            backend_ref: input.work_item_id.to_string(),
            kind: medulla_durable::DurableBackendKind::Jobs,
            durable_status: input.committed_status.to_string(),
            checkpoint_ref: None,
            definition_version: None,
            lease_epoch: Some(input.lease_epoch),
            idempotency_key: None,
        };
        let projection = medulla_durable::project(&mirror, Some(&receipt.node_id));
        for edge in &projection.edges {
            let props = rmp_serde::to_vec_named(&serde_json::json!({
                "relationship": edge.edge_type,
                "tenant": input.tenant,
            }))
            .map_err(|error| error.to_string())?;
            let sealed = crypto.seal(&props);
            let ordinal = next_edge_ordinal(edges, graph, input.work_item_id, &edge.to)?;
            edges.insert(
                (graph, input.work_item_id, edge.to.as_str(), ordinal),
                sealed.as_ref(),
            )?;
        }
    }
    Ok(())
}

/// Every graph node carries its ontology class as its typed label: a stored
/// trace node is stamped with `node_type` (`RunTrace` or `RecursiveTraceNode`)
/// and the committing tenant, so a label read (`GetNodesByLabel`) finds it (R133.6).
fn stamp_class(properties_msgpack: &[u8], tenant: &str, class: &str) -> Result<Vec<u8>, String> {
    let mut properties: std::collections::BTreeMap<String, serde_json::Value> =
        rmp_serde::from_slice(properties_msgpack).map_err(|error| error.to_string())?;
    properties.insert("node_type".into(), class.into());
    properties.insert("tenant".into(), tenant.into());
    rmp_serde::to_vec_named(&properties).map_err(|error| error.to_string())
}

/// Link a recursive trace node to its parent with a `child_of` edge, written in
/// the commit's own transaction (R133.7.2). A root node has no parent and no edge.
fn write_child_of_edge(
    input: &ReceiptRowsInput<'_>,
    receipt: &medulla_result_types::outcome_bundle::ReceiptNode,
    edges: &mut NativeEdgeRows<'_>,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let nodes = decode_recursive_trace_nodes(std::slice::from_ref(receipt), &receipt.run_id)?;
    let Some(parent) = nodes.first().and_then(|n| n.parent_node_id.as_deref()) else {
        return Ok(());
    };
    let props = rmp_serde::to_vec_named(&serde_json::json!({
        "relationship": "child_of",
        "tenant": input.tenant,
    }))
    .map_err(|error| error.to_string())?;
    let sealed = crypto.seal(&props);
    let ordinal = next_edge_ordinal(edges, input.graph, receipt.node_id.as_str(), parent)?;
    edges.insert(
        (input.graph, receipt.node_id.as_str(), parent, ordinal),
        sealed.as_ref(),
    )?;
    Ok(())
}

fn validate_ordinary_completion(
    input: &CommitWorkItemResultInput<'_, '_, '_>,
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(completion) = input.run_completion {
        completion.validate()?;
        run_owner::validate_completion_owner(
            props,
            input.graph,
            input.tenant,
            &completion.run_id,
            input.work_item_id,
        )?;
        if input.outcome_extension.is_some()
            || input.retryable
            || (input.outcome != "succeeded" && input.result_ref.is_some())
        {
            return Err("ordinary completion requires an exclusive final result without failed answer artifacts".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redb_store::graph_row_tables::GraphRowTables;
    use crate::redb_store::work_item::test_shard::{apply, open, with_nodes, with_write, GRAPH};
    use medulla_result_types::outcome_bundle::{
        CommitOutcomeBundle, OutcomeCompleteness, ReceiptNode, ReceiptNodeKind, RunEvent,
        TerminalOutcomeExtension, OUTCOME_BUNDLE_VERSION,
    };
    use sha2::{Digest, Sha256};

    const ITEM: &str = "work-trace";
    const TRACE: &str = "trace:terminal";

    fn digest_for(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn trace_record(run_id: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "run_id": run_id,
            "task_signature": "sig:terminal",
            "model_id": "model:terminal",
            "steps": [{"index": 0, "kind": "tool_call", "action_ref": "tool:fixture", "outcome": "ok"}],
            "outcome": "succeeded",
            "started_at_ms": 1
        })
    }

    fn receipt(
        bundle: &CommitOutcomeBundle,
        kind: ReceiptNodeKind,
        node_id: &str,
        trace_run: &str,
    ) -> ReceiptNode {
        let kind_name = match kind {
            ReceiptNodeKind::RunTrace => "run_trace",
            ReceiptNodeKind::ToolCall => "tool_call",
            ReceiptNodeKind::OutcomeEvaluation => "outcome_evaluation",
            ReceiptNodeKind::ModelRequest => "model_request",
            ReceiptNodeKind::RecursiveTraceNode => "recursive_trace_node",
        };
        let payload_ref = format!("cas:receipt:{node_id}");
        let mut properties = serde_json::json!({
            "node_id": node_id, "kind": kind_name,
            "delegation_id": bundle.delegation_id, "delegator_id": bundle.delegator_id,
            "selected_agent_id": bundle.selected_agent_id,
            "executor_lease_actor": bundle.executor_lease_actor,
            "outcome": bundle.outcome, "work_item_id": bundle.work_item_id,
            "run_id": bundle.run_id, "fence_token": bundle.fence_token,
            "result_ref": bundle.result_ref, "result_digest": bundle.result_digest,
            "event_sequence": bundle.event_sequence, "completeness": bundle.completeness,
            "missing_refs": bundle.missing_refs, "outbox_id": bundle.outbox_id,
            "payload_ref": payload_ref, "capability_digest": bundle.capability_digest,
            "catalog_digest": bundle.catalog_digest, "policy_digest": bundle.policy_digest,
            "model_digest": bundle.model_digest, "payload": {"fixture": true}
        });
        if kind == ReceiptNodeKind::RunTrace {
            properties["run_trace"] = trace_record(trace_run);
        }
        let properties_msgpack = rmp_serde::to_vec_named(&properties).unwrap();
        ReceiptNode {
            node_id: node_id.to_string(),
            kind,
            delegation_id: bundle.delegation_id.clone(),
            work_item_id: bundle.work_item_id.clone(),
            run_id: bundle.run_id.clone(),
            fence_token: bundle.fence_token,
            result_ref: bundle.result_ref.clone(),
            outbox_id: bundle.outbox_id.clone(),
            payload_ref,
            payload_digest: hex::encode(Sha256::digest(&properties_msgpack)),
            properties_msgpack,
        }
    }

    fn extension(
        tag: &str,
        fence: u64,
        outcome: &str,
        trace_run: &str,
    ) -> TerminalOutcomeExtension {
        extension_with(tag, fence, outcome, trace_run, &[])
    }

    /// A recursive trace node receipt carrying `record` under `recursive_trace`.
    fn recursive_receipt(bundle: &CommitOutcomeBundle, record: &serde_json::Value) -> ReceiptNode {
        let id = record["node_id"].as_str().unwrap();
        let mut node = receipt(bundle, ReceiptNodeKind::RecursiveTraceNode, id, "");
        let mut properties: serde_json::Value =
            rmp_serde::from_slice(&node.properties_msgpack).unwrap();
        properties["recursive_trace"] = record.clone();
        node.properties_msgpack = rmp_serde::to_vec_named(&properties).unwrap();
        node.payload_digest = hex::encode(Sha256::digest(&node.properties_msgpack));
        node
    }

    fn trace_node(id: &str, parent: Option<&str>, children: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1, "node_id": id, "run_id": "run:terminal",
            "run_revision": 1, "step_id": format!("step:{id}"), "step_revision": 1, "kind": "code_run",
            "parent_node_id": parent, "child_ids": children,
            "input_refs": [], "output_refs": [],
            "evidence_refs": [{"ref_id": format!("evidence:{id}"), "digest": digest_for('a')}],
            "source_revision": 1, "trace_snapshot_id": "snapshot:1"
        })
    }

    fn extension_with(
        tag: &str,
        fence: u64,
        outcome: &str,
        trace_run: &str,
        recursive: &[serde_json::Value],
    ) -> TerminalOutcomeExtension {
        let bundle = CommitOutcomeBundle {
            schema_version: OUTCOME_BUNDLE_VERSION,
            delegation_id: "delegation:terminal".into(),
            delegator_id: "agent:delegator-a".into(),
            selected_agent_id: "agent:selected-b".into(),
            executor_lease_actor: "worker-a".into(),
            outcome: outcome.into(),
            work_item_id: ITEM.into(),
            fence_token: fence,
            run_id: "run:terminal".into(),
            result_ref: Some("cas:result:terminal".into()),
            result_digest: Some(digest_for('a')),
            artifacts: Vec::new(),
            trace_ref: TRACE.into(),
            tool_call_refs: vec!["toolcall:terminal:0".into()],
            outcome_ref: "outcome:terminal".into(),
            capability_digest: digest_for('b'),
            catalog_digest: digest_for('c'),
            policy_digest: digest_for('d'),
            model_digest: digest_for('e'),
            event_sequence: 1,
            completeness: OutcomeCompleteness::Complete,
            missing_refs: Vec::new(),
            outbox_id: tag.into(),
            langfuse_observation_refs: Vec::new(),
        };
        let receipt_nodes = vec![
            receipt(&bundle, ReceiptNodeKind::RunTrace, TRACE, trace_run),
            receipt(
                &bundle,
                ReceiptNodeKind::ToolCall,
                "toolcall:terminal:0",
                trace_run,
            ),
            receipt(
                &bundle,
                ReceiptNodeKind::OutcomeEvaluation,
                "outcome:terminal",
                trace_run,
            ),
        ];
        let mut receipt_nodes = receipt_nodes;
        receipt_nodes.extend(
            recursive
                .iter()
                .map(|record| recursive_receipt(&bundle, record)),
        );
        let run_event = RunEvent {
            schema_version: OUTCOME_BUNDLE_VERSION,
            delegation_id: "delegation:terminal".into(),
            delegator_id: "agent:delegator-a".into(),
            selected_agent_id: "agent:selected-b".into(),
            executor_lease_actor: "worker-a".into(),
            outcome: outcome.into(),
            work_item_id: ITEM.into(),
            run_id: "run:terminal".into(),
            fence_token: fence,
            outbox_id: tag.into(),
            result_ref: Some("cas:result:terminal".into()),
            capability_digest: digest_for('b'),
            catalog_digest: digest_for('c'),
            policy_digest: digest_for('d'),
            model_digest: digest_for('e'),
            event_sequence: 1,
            completeness: OutcomeCompleteness::Complete,
            missing_refs: Vec::new(),
            kind: "outcome".into(),
            tool_call_ref: None,
            outcome_ref: Some("outcome:terminal".into()),
            payload_digest: digest_for('f'),
            timestamp_ms: 10,
            cursor_token: "cursor:terminal:1".into(),
            carrier_digest: digest_for('0'),
        };
        TerminalOutcomeExtension {
            outcome_bundle: bundle,
            receipt_nodes,
            run_event,
        }
    }

    /// A delegated work item leased to `worker-a` under fence 5.
    fn seed(shard: &medulla_store::redb_store::Shard) {
        let mut row = serde_json::json!({
            "node_type": "WorkItem", "tenant": "tenant-a", "status": "running",
            "lease_owner": "worker-a", "lease_epoch": 4, "fencing_token": 5,
            "lease_expires_at": 20.0, "attempt": 1, "max_attempts": 3,
            "backoff_base_s": 1.0, "downstream_ids": [], "dep_count": 0,
            "next_retry_at": 0.0, "work_item_fence": "",
            "metadata": {
                "delegation_id": "delegation:terminal", "run_id": "run:terminal",
                "agent_id": "agent:selected-b", "capability_digest": digest_for('b')
            },
            "context": {"agent_id": "agent:delegator-a"},
            "catalog_digest": digest_for('c'), "policy_digest": digest_for('d'),
            "model_digest": digest_for('e')
        })
        .as_object()
        .unwrap()
        .clone();
        with_nodes(shard, "seed", |nodes| {
            write_work_item_props(nodes, GRAPH, ITEM, &mut row, DurableCrypto::none())
        });
    }

    fn commit_method(
        tag: &str,
        fence: u64,
        outcome: &str,
        retryable: bool,
        trace_run: &str,
    ) -> Method {
        commit_method_with(tag, fence, outcome, retryable, trace_run, &[])
    }

    fn commit_method_with(
        tag: &str,
        fence: u64,
        outcome: &str,
        retryable: bool,
        trace_run: &str,
        recursive: &[serde_json::Value],
    ) -> Method {
        Method::CommitWorkItemResult {
            tenant: "tenant-a".into(),
            work_item_id: ITEM.into(),
            worker_id: "worker-a".into(),
            lease_epoch: 4,
            fencing_token: fence,
            idempotency_key: format!("key:{tag}"),
            outcome: outcome.into(),
            result_ref: Some("cas:result:terminal".into()),
            outcome_extension: Some(Box::new(extension_with(
                tag, fence, outcome, trace_run, recursive,
            ))),
            run_completion: None,
            error_ref: None,
            retryable,
            now_ms: 1_000,
        }
    }

    fn status_of(payload: medulla_protocol_types::protocol::ResultPayload) -> String {
        let value: serde_json::Value = match payload {
            medulla_protocol_types::protocol::ResultPayload::Json(value) => value,
            medulla_protocol_types::protocol::ResultPayload::Raw(bytes) => {
                rmp_serde::from_slice(&bytes).unwrap()
            }
            other => panic!("unexpected commit payload {other:?}"),
        };
        value["status"].as_str().unwrap_or_default().to_string()
    }

    /// The trace node and the `produced` edge row read from the work item.
    fn read_trace(
        shard: &medulla_store::redb_store::Shard,
    ) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        with_write(shard, "read-trace", 2_000, |admitted| {
            let rows = GraphRowTables::open(admitted.graph(GRAPH)?)?;
            let node = rows
                .nodes
                .get((GRAPH, TRACE))?
                .map(|value| decode_durable(&DurableCrypto::none().unseal(value.value())?))
                .transpose()?;
            let edge = rows
                .edges
                .get((GRAPH, ITEM, TRACE, 0))?
                .map(|value| decode_durable(&DurableCrypto::none().unseal(value.value())?))
                .transpose()?;
            Ok((node, edge))
        })
        .unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.8
    #[test]
    fn run_trace_is_linked_to_unit() {
        let temp = open("run-trace-linked");
        seed(&temp.shard);
        apply(
            &temp.shard,
            "trace-ok",
            commit_method("trace-ok", 5, "succeeded", false, "run:terminal"),
            1_000,
        )
        .unwrap();
        let (node, edge) = read_trace(&temp.shard);
        let node = node.expect("the trace node is stored");
        assert_eq!(node["run_trace"]["run_id"], "run:terminal");
        assert_eq!(node["node_type"], "RunTrace");
        assert_eq!(node["run_trace"]["steps"][0]["action_ref"], "tool:fixture");
        let edge = edge.expect("the unit links to the trace with a produced edge");
        assert_eq!(edge["relationship"], "produced");
        assert_eq!(edge["tenant"], "tenant-a");
    }

    /// The stored node and its `child_of` edge row to `parent`.
    fn read_recursive(
        shard: &medulla_store::redb_store::Shard,
        node: &str,
        parent: &str,
    ) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        with_write(
            shard,
            &format!("read-recursive-{node}-{parent}"),
            2_000,
            |admitted| {
                let rows = GraphRowTables::open(admitted.graph(GRAPH)?)?;
                let stored = rows
                    .nodes
                    .get((GRAPH, node))?
                    .map(|value| decode_durable(&DurableCrypto::none().unseal(value.value())?))
                    .transpose()?;
                let edge = rows
                    .edges
                    .get((GRAPH, node, parent, 0))?
                    .map(|value| decode_durable(&DurableCrypto::none().unseal(value.value())?))
                    .transpose()?;
                Ok((stored, edge))
            },
        )
        .unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.7.2
    #[test]
    fn recursive_trace_nodes_link_to_parents() {
        let temp = open("recursive-trace-linked");
        seed(&temp.shard);
        let nodes = [
            trace_node("rt:root", None, &["rt:a", "rt:b"]),
            trace_node("rt:a", Some("rt:root"), &["rt:c"]),
            trace_node("rt:b", Some("rt:root"), &[]),
            trace_node("rt:c", Some("rt:a"), &[]),
        ];
        apply(
            &temp.shard,
            "recursive-ok",
            commit_method_with(
                "recursive-ok",
                5,
                "succeeded",
                false,
                "run:terminal",
                &nodes,
            ),
            1_000,
        )
        .unwrap();
        for (node, parent) in [("rt:a", "rt:root"), ("rt:b", "rt:root"), ("rt:c", "rt:a")] {
            let (stored, edge) = read_recursive(&temp.shard, node, parent);
            let stored = stored.expect("the recursive trace node is stored");
            assert_eq!(stored["node_type"], "RecursiveTraceNode");
            assert_eq!(stored["recursive_trace"]["parent_node_id"], parent);
            let edge = edge.expect("the node links to its parent");
            assert_eq!(edge["relationship"], "child_of");
            assert_eq!(edge["tenant"], "tenant-a");
        }
        let (root, none) = read_recursive(&temp.shard, "rt:root", "rt:none");
        assert!(
            root.is_some() && none.is_none(),
            "a root has no parent edge"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.7.2
    #[test]
    fn invalid_recursive_snapshot_is_refused_without_state_change() {
        let temp = open("recursive-trace-refused");
        seed(&temp.shard);
        let cycle = [
            trace_node("rt:x", Some("rt:y"), &["rt:y"]),
            trace_node("rt:y", Some("rt:x"), &["rt:x"]),
        ];
        let error = apply(
            &temp.shard,
            "recursive-cycle",
            commit_method_with(
                "recursive-cycle",
                5,
                "succeeded",
                false,
                "run:terminal",
                &cycle,
            ),
            1_000,
        )
        .unwrap_err();
        assert!(error.starts_with("TRACE_CYCLE"), "{error}");
        let orphan = [trace_node("rt:o", Some("rt:missing"), &[])];
        let error = apply(
            &temp.shard,
            "recursive-orphan",
            commit_method_with(
                "recursive-orphan",
                5,
                "succeeded",
                false,
                "run:terminal",
                &orphan,
            ),
            1_000,
        )
        .unwrap_err();
        assert!(error.starts_with("TRACE_INCOMPLETE_ANCESTRY"), "{error}");
        let mut short = trace_node("rt:s", None, &[]);
        short["evidence_refs"][0]["digest"] = "abc".into();
        let error = apply(
            &temp.shard,
            "recursive-short",
            commit_method_with(
                "recursive-short",
                5,
                "succeeded",
                false,
                "run:terminal",
                &[short],
            ),
            1_000,
        )
        .unwrap_err();
        assert!(error.starts_with("TRACE_MISSING_DIGEST"), "{error}");
        let (node, edge) = read_trace(&temp.shard);
        assert!(node.is_none() && edge.is_none(), "nothing is stored");
        assert!(read_recursive(&temp.shard, "rt:x", "rt:y").0.is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.8
    #[test]
    fn run_trace_of_another_run_is_refused_without_state_change() {
        let temp = open("run-trace-foreign-run");
        seed(&temp.shard);
        let error = apply(
            &temp.shard,
            "trace-foreign",
            commit_method("trace-foreign", 5, "succeeded", false, "run:other"),
            1_000,
        )
        .unwrap_err();
        assert!(error.starts_with("RUN_TRACE_INVALID"), "{error}");
        let (node, edge) = read_trace(&temp.shard);
        assert!(node.is_none() && edge.is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R133.8
    #[test]
    fn fenced_or_retry_commit_writes_no_trace() {
        // A stale fence is refused as `fenced`.
        let temp = open("run-trace-fenced");
        seed(&temp.shard);
        let fenced = apply(
            &temp.shard,
            "trace-fenced",
            commit_method("trace-fenced", 4, "succeeded", false, "run:terminal"),
            1_000,
        )
        .unwrap();
        assert_eq!(status_of(fenced), "fenced");
        let (node, edge) = read_trace(&temp.shard);
        assert!(node.is_none() && edge.is_none());

        // A retryable failure with attempts left is `retry_scheduled`.
        let temp = open("run-trace-retry");
        seed(&temp.shard);
        let retry = apply(
            &temp.shard,
            "trace-retry",
            commit_method("trace-retry", 5, "failed", true, "run:terminal"),
            1_000,
        )
        .unwrap();
        assert_eq!(status_of(retry), "retry_scheduled");
        let (node, edge) = read_trace(&temp.shard);
        assert!(node.is_none() && edge.is_none());
    }
}
