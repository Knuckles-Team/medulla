//! The served deploy record methods (MEDULLA-SDLC-R002.7, R002.8) and the approval gate question
//! (MEDULLA-BUSINESS-OPS-R004): `DeployRecordPut`, `DeployRecordGet`, `DeployRecordList` and
//! `ApprovalGateResolve`.
//!
//! Each arm reads only the nodes the caller can see in the request graph, decides in the pure
//! functions of `medulla_source_types::{deploy_record, approval_gate}` and, for the one write,
//! commits the sealed `DeployRecord` node in one gateway-routed batch. A refusal and a replay
//! return before the batch is built, so they write nothing.

use super::*;

use medulla_protocol_types::result_contract::ingestion::{
    ApprovalGateResolve as ApprovalGateResolveResult, DeployRecordGet as DeployRecordGetResult,
    DeployRecordList as DeployRecordListResult, DeployRecordPut as DeployRecordPutResult,
};
use medulla_source_types::approval_gate::{
    resolve_approval_gate, ApprovalGateRequest, ApprovalRule, APPROVAL_RULE_TYPE,
    MAX_APPROVAL_RULES,
};
use medulla_source_types::deploy_record::{
    deploy_record_node_id, handle_deploy_record_get, handle_deploy_record_list,
    handle_deploy_record_put, is_sha256_ref, DeployRecord, DeployRecordInput,
    DEPLOY_RECORD_BODY_PROPERTY, DEPLOY_RECORD_RECORDED_AT_PROPERTY, DEPLOY_RECORD_SEAL_PROPERTY,
    DEPLOY_RECORD_TYPE,
};

/// Most deploy record nodes one list reads.
const MAX_DEPLOY_RECORDS_READ: usize = 100_000;

/// The stored record a node holds, when the node is a deploy record of `tenant`.
fn stored_record(node: &Value, tenant: &str) -> Option<DeployRecord> {
    if node.get("type").and_then(Value::as_str) != Some(DEPLOY_RECORD_TYPE)
        || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
    {
        return None;
    }
    serde_json::from_str(node.get(DEPLOY_RECORD_BODY_PROPERTY)?.as_str()?).ok()
}

/// The nodes of `label` in the request graph that the caller can see, decoded, in id order.
pub(super) async fn visible_nodes_of(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    label: &str,
    bound: usize,
) -> Result<Vec<(String, Value)>, String> {
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let mut nodes: Vec<(String, Value)> = core
        .get_nodes_by_label(label, bound)
        .into_iter()
        .filter(|(node_id, blob)| authority.can_see_node(blob, core.is_schema_node(node_id)))
        .filter_map(|(node_id, blob)| {
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            Some((node_id, node))
        })
        .collect();
    nodes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(nodes)
}

pub(super) fn verified_tenant<'a>(target: &SdlcTarget<'a>) -> Result<&'a str, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    Ok(tenant)
}

/// `DeployRecordPut`: decide against the stored record of the tenant and plan digest, then write
/// the one sealed node. The actor is the verified principal and the time is the engine clock.
pub(super) async fn put(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    input: DeployRecordInput,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let input = input.canonical().map_err(|e| e.to_string())?;
        let node_id = deploy_record_node_id(tenant, &input.plan_digest);
        let stored = visible_node(state, &target, &node_id)
            .await?
            .and_then(|node| stored_record(&node, tenant));
        let write = handle_deploy_record_put(
            stored.as_ref(),
            input,
            target.verified.principal(),
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        )
        .map_err(|e| e.to_string())?;
        if !write.replayed {
            let body = serde_json::to_string(&write.record)
                .map_err(|_| "deploy record encoding failed".to_string())?;
            let mut properties = Map::new();
            properties.insert("type".into(), DEPLOY_RECORD_TYPE.into());
            properties.insert(DEPLOY_RECORD_BODY_PROPERTY.into(), body.into());
            properties.insert(DEPLOY_RECORD_SEAL_PROPERTY.into(), write.record.seal.clone().into());
            properties.insert(
                DEPLOY_RECORD_RECORDED_AT_PROPERTY.into(),
                write.record.recorded_at_ms.into(),
            );
            properties.insert("plan_digest".into(), write.record.record.plan_digest.clone().into());
            properties.insert("target".into(), write.record.record.target.clone().into());
            stamp(&mut properties, &target);
            let operations = bounded(vec![
                json!({"op": "upsert_node", "id": node_id, "properties": properties}),
            ])?;
            commit(state, &target, operations).await?;
        }
        Ok(write.receipt(node_id))
    }
    .await;
    typed_response::<DeployRecordPutResult>(target.req_id, result)
}

/// `DeployRecordGet`: the stored record of one plan digest. A malformed digest, a missing record,
/// a record of another tenant and a record the caller cannot see all refuse alike.
pub(super) async fn get(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    plan_digest: String,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let stored = if is_sha256_ref(&plan_digest) {
            visible_node(state, &target, &deploy_record_node_id(tenant, &plan_digest))
                .await?
                .and_then(|node| stored_record(&node, tenant))
        } else {
            None
        };
        handle_deploy_record_get(stored, &plan_digest).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<DeployRecordGetResult>(target.req_id, result)
}

/// `DeployRecordList`: one page of the tenant's records the caller can see.
pub(super) async fn list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let records: Vec<DeployRecord> =
            visible_nodes_of(state, &target, DEPLOY_RECORD_TYPE, MAX_DEPLOY_RECORDS_READ)
                .await?
                .iter()
                .filter_map(|(_, node)| stored_record(node, tenant))
                .collect();
        handle_deploy_record_list(records, tenant, limit, cursor.as_deref())
            .map_err(|e| e.to_string())
    }
    .await;
    typed_response::<DeployRecordListResult>(target.req_id, result)
}

/// `ApprovalGateResolve`: answer from the `ApprovalRule` nodes of the request graph that the
/// caller can see. A rule the engine cannot read is left out, so its operation answers
/// `unresolved`, never a looser gate.
pub(super) async fn approval_gate_resolve(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    request: ApprovalGateRequest,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        request.validate().map_err(|e| e.to_string())?;
        let rules: Vec<ApprovalRule> =
            visible_nodes_of(state, &target, APPROVAL_RULE_TYPE, MAX_APPROVAL_RULES)
                .await?
                .iter()
                .filter_map(|(node_id, node)| ApprovalRule::from_node(node_id, node, tenant))
                .collect();
        resolve_approval_gate(&request, &rules).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<ApprovalGateResolveResult>(target.req_id, result)
}
