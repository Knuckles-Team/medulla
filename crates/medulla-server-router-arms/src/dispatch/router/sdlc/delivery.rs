//! The served delivery methods (MEDULLA-SDLC-R019.14): `DeliveryOpen`, `DeliveryFeedback`,
//! `DeliveryReport`, `DeliveryStatus`, `DeliveryList`, `DeliveryStop` and `DeliveryRetry`.
//!
//! Each arm reads the deliveries the caller can see in the request graph (a delivery of
//! another tenant is absent, so it refuses the non-disclosing `DELIVERY_NOT_FOUND`), decides
//! in the pure functions of `medulla_sdlc::lifecycle::delivery`, and writes the touched
//! delivery nodes in one gateway-routed batch. A refusal returns before the batch is built.

use super::*;

use std::collections::BTreeSet;

use medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordStored;
use medulla_protocol_types::result_contract::ingestion::{
    DeliveryFeedback as DeliveryFeedbackResult, DeliveryList as DeliveryListResult,
    DeliveryOpen as DeliveryOpenResult, DeliveryReport as DeliveryReportResult,
    DeliveryRespond as DeliveryRespondResult, DeliveryRetry as DeliveryRetryResult,
    DeliveryStatus as DeliveryStatusResult, DeliveryStop as DeliveryStopResult,
};
use medulla_sdlc::lifecycle::delivery::ending::{apply_proposal_event, end_if_done, EndTrigger};
use medulla_sdlc::lifecycle::delivery::feedback::{route_feedback, FeedbackContext};
use medulla_sdlc::lifecycle::delivery::mandate::revalidate_mandate;
use medulla_sdlc::lifecycle::delivery::open::{
    delivery_id, shadow_or_dispatch, DispatchOutcome, OpenDelivery,
};
use medulla_sdlc::lifecycle::delivery::recovery::{
    expired_by_lane_leases, recover_assignments, LaneLease, RecordedEffect,
};
use medulla_sdlc::lifecycle::delivery::rounds::{record_round_verdict, review_round_verdict};
use medulla_sdlc::lifecycle::delivery::stage::{derive_stage, record_answer, record_report};
use medulla_sdlc::lifecycle::delivery::superseded::{
    record_superseded, superseded_verdict, BranchChangeset, CandidateProposal, SupersededVerdict,
};
use medulla_source_types::delivery::{
    decode_delivery, AssignmentState, Delivery, DeliveryAnswer, DeliveryAssignment,
    DeliveryFeedbackReceipt, DeliveryListAnswer, DeliveryListEntry, DeliveryListRequest,
    DeliveryOpenAnswer, DeliveryOpenRequest, DeliveryRef, DeliveryReportWrite,
    DeliveryRespondRequest, DeliveryRetryRequest, FeedbackKind, InputSource, MandateFacts,
    ProposalEvent, ReviewRecord, ReviewerKind, ScmFeedbackEvent, WouldDispatch, DELIVERY_NOT_FOUND,
    DELIVERY_RECORD_PROPERTY, DELIVERY_SCHEMA_VERSION, DELIVERY_TYPE,
};
use medulla_source_types::gate_verdict::{
    gate_verdict_seal, GateVerdictRecord, GATE_VERDICT_RECORD_PROPERTY, GATE_VERDICT_SEAL_PROPERTY,
};
use medulla_source_types::lifecycle::{DefectIntake, FileDefectRequest};

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
    use medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordWrite;
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    let response = medulla_h_decide::decide::log::handle_decision_log(
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
) -> Result<
    (
        Arc<GraphCore>,
        medulla_server_api::carrier::GraphReadAuthority,
    ),
    String,
> {
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

/// Node type of a stored review (MEDULLA-SDLC-R019.9).
const REVIEW_NODE_TYPE: &str = "Review";
/// Node type of a changeset record (MEDULLA-SDLC-R019.12).
const CHANGESET_NODE_TYPE: &str = "Changeset";
/// Node type of an assignment's lane lease (MEDULLA-SDLC-R019.13).
const LEASE_NODE_TYPE: &str = "AssignmentLease";
/// Node type of an outside effect an assignment recorded (MEDULLA-SDLC-R019.13).
const EFFECT_NODE_TYPE: &str = "AssignmentEffect";

/// Every visible node row of one type, ordered by id.
fn rows_of_type(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
    label: &str,
) -> Vec<(String, Map<String, Value>)> {
    let mut rows: Vec<(String, Map<String, Value>)> = core
        .get_nodes_by_label(label, MAX_DELIVERIES_READ)
        .into_iter()
        .filter_map(|(id, blob)| {
            let node = decode_visible(core, authority, &id, &blob, tenant)?;
            Some((id, node))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn string_of(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A `:Review` row as the round budget reads it; a row without a reviewer, a kind or a run key
/// is not a countable round.
fn review_record(row: &Map<String, Value>) -> Option<ReviewRecord> {
    Some(ReviewRecord {
        proposal_id: string_of(row, "proposal_id")?,
        reviewer: string_of(row, "reviewer")?,
        reviewer_kind: serde_json::from_value::<ReviewerKind>(Value::String(string_of(
            row,
            "reviewer_kind",
        )?))
        .ok()?,
        run_key: string_of(row, "run_key")?,
    })
}

/// The per-path content digests a changeset row carries (`path_digests`, a path to digest map).
fn path_digests(row: &Map<String, Value>) -> std::collections::BTreeMap<String, String> {
    row.get("path_digests")
        .and_then(Value::as_object)
        .map(|paths| {
            paths
                .iter()
                .filter_map(|(path, digest)| Some((path.clone(), digest.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The changesets that belong to the proposals `proposal_ids`, by the `proposal_id` their
/// changeset rows carry.
fn changesets_of(
    rows: &[(String, Map<String, Value>)],
    proposal_ids: &[&str],
) -> Vec<BranchChangeset> {
    rows.iter()
        .filter(|(_, row)| {
            string_of(row, "proposal_id").is_some_and(|id| proposal_ids.contains(&id.as_str()))
        })
        .map(|(id, row)| BranchChangeset {
            id: string_of(row, "changeset_id").unwrap_or_else(|| id.clone()),
            path_digests: path_digests(row),
        })
        .collect()
}

fn proposal_event_of(kind: FeedbackKind) -> Option<ProposalEvent> {
    match kind {
        FeedbackKind::ProposalMerged => Some(ProposalEvent::Merged),
        FeedbackKind::ProposalClosed => Some(ProposalEvent::Closed),
        FeedbackKind::ProposalReopened => Some(ProposalEvent::Reopened),
        _ => None,
    }
}

fn follows_proposal(delivery: &Delivery, proposal: &str) -> bool {
    delivery.proposals.iter().any(|p| p.id == proposal)
        || delivery.follows.iter().any(|id| id == proposal)
}

/// The expired leases of running assignments and the outside effects assignments recorded,
/// read from the `AssignmentLease` and `AssignmentEffect` rows of the graph.
fn lease_facts(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    tenant: &str,
    now: i64,
) -> (BTreeSet<String>, Vec<RecordedEffect>) {
    let expired = rows_of_type(core, authority, tenant, LEASE_NODE_TYPE)
        .into_iter()
        .filter(|(_, row)| {
            row.get("expires_at_ms")
                .and_then(Value::as_i64)
                .is_some_and(|at| at < now)
        })
        .filter_map(|(_, row)| string_of(&row, "assignment_id"))
        .collect();
    let effects = rows_of_type(core, authority, tenant, EFFECT_NODE_TYPE)
        .into_iter()
        .filter_map(|(_, row)| {
            Some(RecordedEffect {
                assignment_id: string_of(&row, "assignment_id")?,
                description: string_of(&row, "description")?,
                receipt: string_of(&row, "receipt"),
            })
        })
        .collect();
    (expired, effects)
}

/// Recover the assignments of `delivery` whose lease expired (MEDULLA-SDLC-R019.13): the run
/// trace of a replacement is derived from the ended assignment's id, so a second recovery finds
/// the replacement already there.
fn recovered(
    delivery: &Delivery,
    expired: &BTreeSet<String>,
    effects: &[RecordedEffect],
    now: i64,
) -> Delivery {
    recover_assignments(
        delivery,
        expired,
        effects,
        &|assignment_id| format!("recovery:{assignment_id}"),
        now,
    )
}

/// The lane leases of the verified tenant, read through the served `DevelopmentLaneStatus`
/// under the caller's own authority (MEDULLA-SDLC-R019.13). Pages until the lane store reports
/// the status complete.
async fn lane_leases(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<Vec<LaneLease>, String> {
    use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
    use medulla_result_types::epistemic_operations::{
        DevelopmentLaneHoldState, DevelopmentLaneStatusRequest,
        DevelopmentLaneStatusRequestSchemaVersion, DevelopmentLaneStatusResult,
    };
    const PAGE: u64 = 500;
    const MAX_PAGES: usize = 1_000;
    let mut leases = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = Request {
            id: target.req_id,
            graph: target.graph.to_owned(),
            auth_token: String::new(),
            agent_id: target.caller.map(str::to_owned),
            method: Method::DevelopmentLaneStatus {
                request: DevelopmentLaneStatusRequest {
                    schema_version: DevelopmentLaneStatusRequestSchemaVersion::V1,
                    tenant_ref: target.verified.tenant().to_owned(),
                    hold_id: None,
                    lane_id: None,
                    work_item_id: None,
                    limit: PAGE,
                    cursor: cursor.clone(),
                    now_ms: 0,
                },
            },
            trace_id: None,
        };
        let response = medulla_server_api::api::reentry::dispatch_verified_request(
            state,
            request,
            target.verified.clone(),
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let Some(ResultPayload::Raw(bytes)) = response.result else {
            return Err("the lane status answered an unexpected payload".to_string());
        };
        let page: DevelopmentLaneStatusResult = rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("the lane status is unreadable: {error}"))?;
        leases.extend(page.holds.iter().map(|hold| LaneLease {
            work_item_id: hold.work_item_id.clone(),
            expires_at_ms: i64::try_from(hold.expires_at_ms).unwrap_or(i64::MAX),
            active: hold.state == DevelopmentLaneHoldState::Active,
        }));
        match page.next_cursor {
            Some(next) if !page.complete => cursor = Some(next),
            _ => return Ok(leases),
        }
    }
    Err("the lane status holds more pages than one recovery reads".to_string())
}

/// Recover the assignments of every delivery of the request graph whose lease lapsed
/// (MEDULLA-SDLC-R019.13), once, at engine start. An assignment lapsed when the lane store's
/// lease for it expired (mapped by delivery id and run trace) or its `AssignmentLease` row did.
/// The ended assignments, their replacements and the moved inputs commit in one batch. Answers
/// how many deliveries changed; a second run changes none.
pub(in crate::dispatch::router) async fn recover_at_boot(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
) -> Result<u32, String> {
    require_tenant(&target)?;
    let tenant = target.verified.tenant().to_owned();
    let (core, authority) = read_graph(state, &target).await?;
    let deliveries = load_deliveries(&core, &authority, &tenant);
    if !deliveries.iter().any(|delivery| {
        delivery
            .assignments
            .iter()
            .any(|assignment| assignment.state == AssignmentState::Running)
    }) {
        return Ok(0);
    }
    let now = now_ms();
    let (mut expired, effects) = lease_facts(&core, &authority, &tenant, now);
    let leases = lane_leases(state, &target).await?;
    expired.extend(expired_by_lane_leases(&deliveries, &leases, now));
    let mut operations = Vec::new();
    for delivery in &deliveries {
        let next = recovered(delivery, &expired, &effects, now);
        if next != *delivery {
            operations.push(delivery_operation(&next, &target)?);
        }
    }
    if operations.is_empty() {
        return Ok(0);
    }
    let changed = operations.len() as u32;
    commit(state, &target, bounded(operations)?).await?;
    Ok(changed)
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
                    commit(
                        state,
                        &target,
                        vec![delivery_operation(&delivery, &target)?],
                    )
                    .await?;
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
        // actor's `source:write` is the admission arbora stamped on the event: the call's own
        // scopes belong to the intake principal, not to the code-host actor (R019.6).
        let actor_can_write = event
            .admitted_principal
            .as_deref()
            .is_some_and(|principal| !principal.trim().is_empty());
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
        let now = now_ms();
        let mut routed = routing.deliveries.clone();
        // R019.13: a delivery that receives an event first recovers assignments whose lease
        // expired, so the event reaches a live assignment and no effect repeats blindly.
        let (expired, effects) = lease_facts(&core, &authority, &tenant, now);
        for delivery in routed.iter_mut() {
            *delivery = recovered(delivery, &expired, &effects, now);
        }
        // R019.11: a proposal merge, close or reopen fires the proposal machine, and the last
        // close ends the delivery with its outcome.
        if let Some(proposal_event) = proposal_event_of(event.kind) {
            for delivery in routed.iter_mut() {
                let applied = apply_proposal_event(delivery, &event.proposal_id, proposal_event)
                    .map_err(|refused| refused.to_string())?;
                *delivery = if proposal_event == ProposalEvent::Reopened {
                    applied
                } else {
                    end_if_done(&applied, EndTrigger::ProposalClosed(proposal_event), now)
                };
            }
        }
        // R019.9: an automated review is one round of its proposal and reviewer; the verdict
        // joins the delivery as an input.
        if event.kind == FeedbackKind::ReviewSubmitted && event.source == InputSource::Automation {
            let reviews: Vec<ReviewRecord> =
                rows_of_type(&core, &authority, &tenant, REVIEW_NODE_TYPE)
                    .into_iter()
                    .filter_map(|(_, row)| review_record(&row))
                    .collect();
            let current = rows_of_type(&core, &authority, &tenant, REVIEW_NODE_TYPE)
                .into_iter()
                .find(|(_, row)| {
                    string_of(row, "event_id").as_deref() == Some(event.event_id.as_str())
                })
                .and_then(|(_, row)| review_record(&row));
            if let Some(current) = current {
                let max_rounds = medulla_server_api::knob_config::knobs()
                    .sdlc_delivery_max_automated_review_rounds;
                let verdict = review_round_verdict(&reviews, &current, max_rounds);
                for delivery in routed.iter_mut() {
                    *delivery = record_round_verdict(delivery, &event.event_id, verdict);
                }
            }
        }
        // R019.12: a person's proposal that carries the delivery branch supersedes the delivery.
        let mut superseded: Vec<Delivery> = Vec::new();
        if matches!(
            event.kind,
            FeedbackKind::HeadPushed | FeedbackKind::ProposalMerged
        ) {
            let changesets = rows_of_type(&core, &authority, &tenant, CHANGESET_NODE_TYPE);
            let held = changesets_of(&changesets, &[event.proposal_id.as_str()]);
            let candidate = CandidateProposal {
                proposal_id: event.proposal_id.clone(),
                by_person: event.source == InputSource::Person,
                changeset_ids: held.iter().map(|c| c.id.clone()).collect(),
                path_digests: held.iter().flat_map(|c| c.path_digests.clone()).collect(),
            };
            for delivery in deliveries
                .iter()
                .filter(|d| d.ended.is_none() && !follows_proposal(d, &event.proposal_id))
            {
                let ids: Vec<&str> = delivery.proposals.iter().map(|p| p.id.as_str()).collect();
                let branch = changesets_of(&changesets, &ids);
                if superseded_verdict(&branch, &candidate) == SupersededVerdict::CloseSuperseded {
                    superseded.push(record_superseded(
                        delivery,
                        &event.event_id,
                        &event.proposal_id,
                    ));
                }
            }
        }
        routed.extend(superseded);
        let mut properties = Map::new();
        properties.insert("type".into(), FEEDBACK_EVENT_TYPE.into());
        properties.insert("event_id".into(), event.event_id.clone().into());
        properties.insert("connector".into(), event.connector.clone().into());
        properties.insert("proposal_id".into(), event.proposal_id.clone().into());
        properties.insert(
            "record".into(),
            serde_json::to_string(&event)
                .map_err(|e| e.to_string())?
                .into(),
        );
        stamp(&mut properties, &target);
        let mut operations =
            vec![json!({"op": "upsert_node", "id": event_node, "properties": properties})];
        for delivery in &routed {
            operations.push(delivery_operation(delivery, &target)?);
        }
        // MEDULLA-SDLC-R017.4: the merge of the pull request an approved proposal delivered
        // moves that proposal to `landed`. Only an approved proposal moves; the write merges
        // `status` and leaves the owner and the content as `ProposalPut` wrote them.
        if event.kind == FeedbackKind::ProposalMerged {
            let approved = visible_object(&core, &authority, &tenant, &event.proposal_id)
                .is_some_and(|node| {
                    node.get("type").and_then(Value::as_str) == Some(PROPOSAL_TYPE)
                        && node.get("status").and_then(Value::as_str)
                            == Some(ProposalState::Approved.stored())
                });
            if approved {
                let mut landed = Map::new();
                landed.insert("type".into(), PROPOSAL_TYPE.into());
                landed.insert("tenant_id".into(), tenant.clone().into());
                landed.insert("status".into(), ProposalState::Landed.stored().into());
                operations.push(
                    json!({"op": "upsert_node", "id": event.proposal_id, "properties": landed}),
                );
            }
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
            delivered_to: routed.iter().map(|d| d.id.clone()).collect(),
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
        let delivery = find_delivery(
            &load_deliveries(&core, &authority, &tenant),
            &write.delivery_id,
        )?;
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
            .filter(|delivery| {
                request
                    .repo
                    .as_ref()
                    .is_none_or(|repo| &delivery.repo == repo)
            })
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

/// `DeliveryRespond` (MEDULLA-SDLC-R019.17): a person's answer to the open question.
pub(super) async fn respond(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: DeliveryRespondRequest,
) -> Response {
    let result = async {
        require_tenant(&target)?;
        let (core, authority) = read_graph(state, &target).await?;
        let delivery = find_delivery(
            &load_deliveries(&core, &authority, target.verified.tenant()),
            &request.delivery_id,
        )?;
        let answered = record_answer(&delivery, &request.answer)?;
        commit(
            state,
            &target,
            vec![delivery_operation(&answered, &target)?],
        )
        .await?;
        Ok(answer(answered))
    }
    .await;
    typed_response::<DeliveryRespondResult>(target.req_id, result)
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
        // R019.13: assignments whose lease expired end with `host_restarted` and are replaced
        // by a pending one that carries the recovery note, so the retry never repeats an
        // outside effect blindly. A second call finds the replacement already there.
        let now = now_ms();
        let (expired, effects) = lease_facts(&core, &authority, target.verified.tenant(), now);
        let before = delivery.clone();
        delivery = recovered(&delivery, &expired, &effects, now);
        if delivery != before {
            commit(
                state,
                &target,
                vec![delivery_operation(&delivery, &target)?],
            )
            .await?;
            return Ok(answer(delivery));
        }
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
            commit(
                state,
                &target,
                vec![delivery_operation(&delivery, &target)?],
            )
            .await?;
        }
        Ok(answer(delivery))
    }
    .await;
    typed_response::<DeliveryRetryResult>(target.req_id, result)
}
