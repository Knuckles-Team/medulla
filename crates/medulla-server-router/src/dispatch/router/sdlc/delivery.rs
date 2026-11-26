//! The served delivery methods (MEDULLA-SDLC-R019.14): `DeliveryOpen`, `DeliveryFeedback`,
//! `DeliveryReport`, `DeliveryStatus`, `DeliveryList`, `DeliveryStop` and `DeliveryRetry`.
//!
//! Each arm reads the deliveries the caller can see in the request graph (a delivery of
//! another tenant is absent, so it refuses the non-disclosing `DELIVERY_NOT_FOUND`), decides
//! in the pure functions of `medulla_sdlc::lifecycle::delivery`, and writes the touched
//! delivery nodes in one gateway-routed batch. A refusal returns before the batch is built.

use super::*;

use std::collections::BTreeSet;

use medulla_sdlc::lifecycle::delivery::ending::{end_if_done, EndTrigger};
use medulla_sdlc::lifecycle::delivery::feedback::{route_feedback, FeedbackContext};
use medulla_sdlc::lifecycle::delivery::mandate::revalidate_mandate;
use medulla_sdlc::lifecycle::delivery::open::{
    delivery_id, shadow_or_dispatch, DispatchOutcome, OpenDelivery,
};
use medulla_sdlc::lifecycle::delivery::stage::{derive_stage, record_report};
use medulla_source_types::delivery::{
    decode_delivery, AssignmentState, Delivery, DeliveryAnswer, DeliveryAssignment,
    DeliveryFeedbackReceipt, DeliveryListAnswer, DeliveryListEntry, DeliveryListRequest,
    DeliveryOpenAnswer, DeliveryOpenRequest, DeliveryRef, DeliveryReportWrite,
    DeliveryRetryRequest, MandateFacts, ScmFeedbackEvent, WouldDispatch, DELIVERY_NOT_FOUND,
    DELIVERY_RECORD_PROPERTY, DELIVERY_SCHEMA_VERSION, DELIVERY_TYPE,
};
use medulla_decision_types::decision::statistical::lifecycle_record::LifecycleRecordStored;
use medulla_source_types::gate_verdict::{
    gate_verdict_seal, GateVerdictRecord, GATE_VERDICT_RECORD_PROPERTY,
    GATE_VERDICT_SEAL_PROPERTY,
};
use medulla_source_types::lifecycle::{DefectIntake, FileDefectRequest};
use medulla_protocol_types::result_contract::ingestion::{
    DeliveryFeedback as DeliveryFeedbackResult, DeliveryList as DeliveryListResult,
    DeliveryOpen as DeliveryOpenResult, DeliveryReport as DeliveryReportResult,
    DeliveryRetry as DeliveryRetryResult, DeliveryStatus as DeliveryStatusResult,
    DeliveryStop as DeliveryStopResult,
};

const MAX_DELIVERIES_READ: usize = 100_000;
const FEEDBACK_EVENT_TYPE: &str = "FeedbackEvent";
const CONNECTOR_TYPE: &str = "Connector";

/// Commit one would-dispatch decision through `DecisionLog`, the one decision mechanism. The
/// caller's verified context decides whether the log write is admitted.
async fn commit_would_dispatch(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    record: WouldDispatch,
) -> Result<LifecycleRecordStored, String> {
    use medulla_decision_types::decision::statistical::lifecycle_record::LifecycleRecordWrite;
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    let response = medulla_h_decide::decide::handle_decision_log(
        state,
        target.req_id,
        target.verified,
        DecisionLogOp::LifecycleRecord {
            tenant_id: target.verified.tenant().to_owned(),
            record: LifecycleRecordWrite::WouldDispatch { record },
        },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(medulla_protocol_types::protocol::ResultPayload::Json(value)) => {
            serde_json::from_value(value).map_err(|error| error.to_string())
        }
        _ => Err("INVALID_ARGUMENT: the decision log answered no stored record".to_string()),
    }
}

fn require_tenant(target: &SdlcTarget<'_>) -> Result<(), String> {
    if target.verified.tenant().trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    Ok(())
}

fn not_found(id: &str) -> String {
    format!("{DELIVERY_NOT_FOUND}: {id}")
}

fn now_ms() -> i64 {
    i64::try_from(medulla_kernel::kernel::dispatch_support::authoritative_now_ms())
        .unwrap_or(i64::MAX)
}

/// The caller's view of one graph: the core and its read authority.
async fn read_graph(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<(Arc<GraphCore>, medulla_server_api::carrier::GraphReadAuthority), String> {
    ensure_graph_resident(state, target.graph).await;
    let current = timed_read(state).await;
    authorized_graph_read(
        &current,
        target.graph,
        target.verified,
        &format!("GRAPH_NOT_FOUND: {}", target.graph),
    )
}

/// The stored node `id` as a visible, tenant-owned object.
fn visible_object(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
    id: &str,
) -> Option<Map<String, Value>> {
    let blob = core.get_node_properties(id)?;
    if !authority.can_see_node(&blob, core.is_schema_node(id)) {
        return None;
    }
    let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(&blob) else {
        return None;
    };
    if node
        .get("tenant_id")
        .and_then(Value::as_str)
        .is_some_and(|owner| owner != tenant)
    {
        return None;
    }
    Some(node)
}

/// Every delivery of the tenant the caller can see, ordered by id.
fn load_deliveries(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
) -> Vec<Delivery> {
    let mut deliveries: Vec<Delivery> = core
        .get_nodes_by_label(DELIVERY_TYPE, MAX_DELIVERIES_READ)
        .into_iter()
        .filter(|(id, blob)| authority.can_see_node(blob, core.is_schema_node(id)))
        .filter_map(|(_, blob)| {
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            let record = node.get(DELIVERY_RECORD_PROPERTY)?.as_str()?;
            let delivery = decode_delivery(serde_json::from_str(record).ok()?).ok()?;
            (delivery.tenant == tenant).then_some(delivery)
        })
        .collect();
    deliveries.sort_by(|left, right| left.id.cmp(&right.id));
    deliveries
}

/// A visible node's decoded properties, by the blob the core holds for it.
fn decode_visible(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    id: &str,
    blob: &[u8],
    tenant: &str,
) -> Option<Map<String, Value>> {
    if !authority.can_see_node(blob, core.is_schema_node(id)) {
        return None;
    }
    let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(blob) else {
        return None;
    };
    node.get("tenant_id")
        .and_then(Value::as_str)
        .is_none_or(|owner| owner == tenant)
        .then_some(node)
}

/// The connectors the caller can see in the connector registry: every visible `Connector` node,
/// by its `name` and by its id with a `connector:` prefix or any path or fragment removed.
fn registered_connectors(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (id, blob) in core.get_nodes_by_label(CONNECTOR_TYPE, MAX_DELIVERIES_READ) {
        let Some(node) = decode_visible(core, authority, &id, &blob, tenant) else {
            continue;
        };
        if let Some(name) = node.get("name").and_then(Value::as_str) {
            names.insert(name.to_owned());
        }
        let bare = id.strip_prefix("connector:").unwrap_or(&id);
        names.insert(bare.to_owned());
        if let Some(tail) = bare.rsplit(['/', '#']).next() {
            names.insert(tail.to_owned());
        }
    }
    names
}

/// The sealed gate verdicts the caller can see: the node id of every `GateVerdict` whose record
/// decodes and whose stored seal equals the seal of that record. A node without a valid seal
/// is not evidence.
fn sealed_gate_verdicts(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
) -> Vec<(String, GateVerdictRecord)> {
    let mut sealed = Vec::new();
    for (id, blob) in core.get_nodes_by_label(GATE_VERDICT_TYPE, MAX_DELIVERIES_READ) {
        let Some(node) = decode_visible(core, authority, &id, &blob, tenant) else {
            continue;
        };
        let record = node
            .get(GATE_VERDICT_RECORD_PROPERTY)
            .and_then(Value::as_str)
            .and_then(|text| serde_json::from_str::<GateVerdictRecord>(text).ok());
        let seal = node.get(GATE_VERDICT_SEAL_PROPERTY).and_then(Value::as_str);
        if let (Some(record), Some(seal)) = (record, seal) {
            if gate_verdict_seal(&record) == seal {
                sealed.push((id, record));
            }
        }
    }
    sealed.sort_by(|a, b| a.0.cmp(&b.0));
    sealed
}

fn find_delivery(deliveries: &[Delivery], id: &str) -> Result<Delivery, String> {
    deliveries
        .iter()
        .find(|delivery| delivery.id == id)
        .cloned()
        .ok_or_else(|| not_found(id))
}

/// The one `Delivery` node, stamped with the verified tenant and owner.
fn delivery_operation(delivery: &Delivery, target: &SdlcTarget<'_>) -> Result<Value, String> {
    let record = serde_json::to_string(delivery).map_err(|e| e.to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), DELIVERY_TYPE.into());
    properties.insert("repo".into(), delivery.repo.clone().into());
    properties.insert("slice_root".into(), delivery.slice_root.clone().into());
    properties.insert(DELIVERY_RECORD_PROPERTY.into(), record.into());
    stamp(&mut properties, target);
    Ok(json!({"op": "upsert_node", "id": delivery.id, "properties": properties}))
}

fn answer(delivery: Delivery) -> DeliveryAnswer {
    let stage = derive_stage(&delivery).as_str().to_owned();
    DeliveryAnswer { delivery, stage }
}

/// `DeliveryOpen` (MEDULLA-SDLC-R019.3, R019.4).
pub(super) async fn open(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryOpenRequest,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let blank = |text: &str| text.trim().is_empty();
        if blank(&request.repo)
            || blank(&request.slice_root)
            || blank(&request.dispatch_decision_id)
            || blank(&request.run_trace_id)
        {
            return Err(LifecycleRefusal::InvalidArgument(
                "a delivery open names its repo, slice root, dispatch decision and run".into(),
            )
            .to_string());
        }
        let tenant = target.verified.tenant().to_owned();
        let mode = stored_project_mode(state, &target, &request.repo).await?;
        let (core, authority) = read_graph(state, &target).await?;
        let deliveries = load_deliveries(&core, &authority, &tenant);
        let key = delivery_id(&tenant, &request.dispatch_decision_id, &request.slice_root);
        let existing = deliveries.into_iter().find(|delivery| delivery.id == key);
        let reopened = existing.is_some();
        let input = OpenDelivery {
            tenant: &tenant,
            repo: &request.repo,
            slice_root: &request.slice_root,
            scope_rows: &request.scope_rows,
            dispatch_decision_id: &request.dispatch_decision_id,
            policy_version: &request.policy_version,
            actor: target.verified.principal(),
            origin: request.origin,
            mode,
            run_trace_id: &request.run_trace_id,
            now_ms: now_ms(),
        };
        match shadow_or_dispatch(&input, existing, &request.evidence_digests)
            .map_err(|e| e.to_string())?
        {
            DispatchOutcome::Dispatched(delivery) => {
                if !reopened {
                    commit(state, &target, vec![delivery_operation(&delivery, &target)?]).await?;
                }
                Ok(DeliveryOpenAnswer {
                    delivery: Some(*delivery),
                    would_dispatch: None,
                    reopened,
                })
            }
            DispatchOutcome::WouldDispatch(record) => {
                // The one decision mechanism (MEDULLA-SDLC-R019.4): the would-dispatch
                // decision is a record of `DecisionLog`, not a node of a second log.
                let stored = commit_would_dispatch(state, &target, record.clone()).await?;
                Ok(DeliveryOpenAnswer {
                    delivery: None,
                    would_dispatch: Some(record),
                    reopened: stored.replayed,
                })
            }
        }
    }
    .await;
    typed_response::<DeliveryOpenResult>(target.req_id, result)
}

/// `DeliveryFeedback` (MEDULLA-SDLC-R019.5, R019.6).
pub(super) async fn feedback(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    event: ScmFeedbackEvent,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let tenant = target.verified.tenant().to_owned();
        let (core, authority) = read_graph(state, &target).await?;
        let deliveries = load_deliveries(&core, &authority, &tenant);
        let event_node = format!("feedback:{}", event.event_id);
        let already_stored = visible_object(&core, &authority, &tenant, &event_node).is_some();
        let registered = registered_connectors(&core, &authority, &tenant);
        let proposal_known = deliveries.iter().any(|delivery| {
            delivery.proposals.iter().any(|p| p.id == event.proposal_id)
                || delivery.follows.iter().any(|id| id == &event.proposal_id)
        }) || visible_object(&core, &authority, &tenant, &event.proposal_id)
            .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some(PROPOSAL_TYPE));
        // A person's event may resume an ended delivery whose mandate still holds. The
        // actor's `source:write` is read from the verified scopes of this call.
        let actor_can_write = target.verified.allows_method("source:write", true);
        let mut revalidated = BTreeSet::new();
        for delivery in deliveries.iter().filter(|d| d.ended.is_some()) {
            let mode = stored_project_mode(state, &target, &delivery.repo).await?;
            let rows_open = delivery.scope_rows.iter().all(|row| {
                visible_object(
                    &core,
                    &authority,
                    &tenant,
                    &medulla_h_ingest::source_ingestion::spec_projection::requirement_node_id(
                        &delivery.repo,
                        row,
                    ),
                )
                .is_some_and(|node| {
                    node.get("rowStatus").and_then(Value::as_str) != Some("VERIFIED")
                })
            });
            let facts = MandateFacts {
                mode,
                rows_open,
                actor_can_write,
                policy_version: delivery.mandate.policy_version.clone(),
            };
            if revalidate_mandate(delivery, &facts).is_ok() {
                revalidated.insert(delivery.id.clone());
            }
        }
        let routing = route_feedback(
            &event,
            &deliveries,
            &FeedbackContext {
                registered_connectors: &registered,
                proposal_known,
                already_stored,
                revalidated: &revalidated,
            },
        )
        .map_err(|e| e.to_string())?;
        if routing.duplicate {
            return Ok(DeliveryFeedbackReceipt {
                duplicate: true,
                delivered_to: Vec::new(),
                resumed: Vec::new(),
                defect_filed: false,
            });
        }
        let mut properties = Map::new();
        properties.insert("type".into(), FEEDBACK_EVENT_TYPE.into());
        properties.insert("event_id".into(), event.event_id.clone().into());
        properties.insert("connector".into(), event.connector.clone().into());
        properties.insert("proposal_id".into(), event.proposal_id.clone().into());
        properties.insert(
            "record".into(),
            serde_json::to_string(&event).map_err(|e| e.to_string())?.into(),
        );
        stamp(&mut properties, &target);
        let mut operations =
            vec![json!({"op": "upsert_node", "id": event_node, "properties": properties})];
        for delivery in &routing.deliveries {
            operations.push(delivery_operation(delivery, &target)?);
        }
        if routing.file_defect {
            let request = FileDefectRequest {
                repo: target.graph.to_owned(),
                intake: DefectIntake::ConnectorIssue,
                title: format!("failed check on {}", event.proposal_id),
                severity: None,
                source_record_id: event.event_id.clone(),
                observed_at: u64::try_from(event.observed_at).unwrap_or(0),
                text: event.summary.clone(),
            };
            let request = handle_file_defect(request).map_err(|e| e.to_string())?;
            let write = file_defect_write(state, &target, request).await?;
            operations.extend(defect_operations(&write, &target)?);
        }
        let operations = bounded(operations)?;
        commit(state, &target, operations).await?;
        Ok(DeliveryFeedbackReceipt {
            duplicate: false,
            delivered_to: routing.deliveries.iter().map(|d| d.id.clone()).collect(),
            resumed: routing.resumed,
            defect_filed: routing.file_defect,
        })
    }
    .await;
    typed_response::<DeliveryFeedbackResult>(target.req_id, result)
}

/// `DeliveryReport` (MEDULLA-SDLC-R019.7): the gate verdicts the caller can see are the
/// evidence a `ready` report may name.
pub(super) async fn report(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    write: DeliveryReportWrite,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let tenant = target.verified.tenant().to_owned();
        let (core, authority) = read_graph(state, &target).await?;
        let delivery = find_delivery(&load_deliveries(&core, &authority, &tenant), &write.delivery_id)?;
        let known_verdicts: BTreeSet<String> = sealed_gate_verdicts(&core, &authority, &tenant)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let recorded =
            record_report(&delivery, &write, &known_verdicts).map_err(|e| e.to_string())?;
        let ended = end_if_done(&recorded, EndTrigger::Report, now_ms());
        commit(state, &target, vec![delivery_operation(&ended, &target)?]).await?;
        Ok(answer(ended))
    }
    .await;
    typed_response::<DeliveryReportResult>(target.req_id, result)
}

/// `DeliveryStatus` (MEDULLA-SDLC-R019.14): one delivery with its derived stage.
pub(super) async fn status(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryRef,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let (core, authority) = read_graph(state, &target).await?;
        let deliveries = load_deliveries(&core, &authority, target.verified.tenant());
        find_delivery(&deliveries, &request.delivery_id).map(answer)
    }
    .await;
    typed_response::<DeliveryStatusResult>(target.req_id, result)
}

/// `DeliveryList` (MEDULLA-SDLC-R019.14): the deliveries of the tenant, by id.
pub(super) async fn list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryListRequest,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let (core, authority) = read_graph(state, &target).await?;
        let deliveries = load_deliveries(&core, &authority, target.verified.tenant())
            .into_iter()
            .filter(|delivery| request.repo.as_ref().is_none_or(|repo| &delivery.repo == repo))
            .map(|delivery| DeliveryListEntry {
                stage: derive_stage(&delivery).as_str().to_owned(),
                id: delivery.id,
                repo: delivery.repo,
                slice_root: delivery.slice_root,
                ended: delivery.ended,
            })
            .collect();
        Ok(DeliveryListAnswer { deliveries })
    }
    .await;
    typed_response::<DeliveryListResult>(target.req_id, result)
}

/// `DeliveryStop` (MEDULLA-SDLC-R019.11): a person's stop ends the delivery `stopped`.
pub(super) async fn stop(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryRef,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let (core, authority) = read_graph(state, &target).await?;
        let delivery = find_delivery(
            &load_deliveries(&core, &authority, target.verified.tenant()),
            &request.delivery_id,
        )?;
        let ended = end_if_done(&delivery, EndTrigger::Stopped, now_ms());
        if ended != delivery {
            commit(state, &target, vec![delivery_operation(&ended, &target)?]).await?;
        }
        Ok(answer(ended))
    }
    .await;
    typed_response::<DeliveryStopResult>(target.req_id, result)
}

/// `DeliveryRetry`: a new pending assignment on a live delivery whose assignments all ended.
pub(super) async fn retry(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryRetryRequest,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        if request.run_trace_id.trim().is_empty() {
            return Err(LifecycleRefusal::InvalidArgument(
                "a retry names the run arbora minted".into(),
            )
            .to_string());
        }
        let (core, authority) = read_graph(state, &target).await?;
        let mut delivery = find_delivery(
            &load_deliveries(&core, &authority, target.verified.tenant()),
            &request.delivery_id,
        )?;
        let busy = delivery
            .assignments
            .iter()
            .any(|a| a.state != AssignmentState::Ended);
        if delivery.ended.is_some() || busy {
            return Err(LifecycleRefusal::InvalidArgument(
                "a retry needs a live delivery whose assignments all ended".into(),
            )
            .to_string());
        }
        let id = format!(
            "assignment:{}",
            &medulla_source_types::lifecycle::sha256_hex(
                format!("{}\n{}", delivery.id, request.run_trace_id).as_bytes()
            )[..32]
        );
        if !delivery.assignments.iter().any(|a| a.id == id) {
            delivery.assignments.push(DeliveryAssignment {
                schema_version: DELIVERY_SCHEMA_VERSION,
                id,
                run_trace_id: request.run_trace_id.clone(),
                state: AssignmentState::Pending,
                report: None,
                started_at: None,
                ended_at: None,
                ended_reason: None,
                recovery: None,
            });
            commit(state, &target, vec![delivery_operation(&delivery, &target)?]).await?;
        }
        Ok(answer(delivery))
    }
    .await;
    typed_response::<DeliveryRetryResult>(target.req_id, result)
}
