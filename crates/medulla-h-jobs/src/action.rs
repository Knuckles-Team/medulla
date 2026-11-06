//! Served governed actions (MEDULLA-UNIFIED-DATA-PLANE-R700.3).
//!
//! `ActionRegister` validates an [`ActionSpec`] and stores it as the graph node
//! `action-spec:<name>`. `ActionSubmit` loads that node, runs
//! [`ActionSpec::submit`] and commits ONE `BatchUpdate` through the request
//! boundary: every applied effect plus the `ActionRun` node (the verdicts and
//! the parameter digest). A refused run commits only its `ActionRun` node and
//! answers with the refusal's registered code. A `BatchUpdate` emits no
//! per-row change events, so after the batch commits the handler emits the
//! outbox itself: the `ActionRun` node's change event (its post-image is the
//! payload) and the `__action_completed` topic marker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination::{ActionRegister, ActionSubmit};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_work_types::action_spec::{
    ActionRefusal, ActionRun, ActionSpec, ActionStatus, EffectDisposition, EffectKind, ParamValue,
    ResolvedEffect,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const SPEC_PREFIX: &str = "action-spec:";
const SPEC_BODY: &str = "spec_body";
const RUN_BODY: &str = "action_run";

/// Already-authorized graph and caller context for one action call.
pub struct HandleContext<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub req_id: u64,
    pub graph_name: &'a str,
    pub caller: Option<&'a str>,
    pub verified_context: &'a VerifiedRequestContext,
    pub core: &'a Arc<GraphCore>,
}

/// Handle `ActionRegister` and `ActionSubmit`; hand any other method back.
pub async fn try_handle(ctx: HandleContext<'_>, method: Method) -> Result<Response, Method> {
    let id = ctx.req_id;
    match method {
        Method::ActionRegister { spec } => Ok(typed::<ActionRegister>(
            id,
            handle_action_register(&ctx, spec).await,
        )),
        Method::ActionSubmit { action, params } => Ok(typed::<ActionSubmit>(
            id,
            handle_action_submit(&ctx, &action, params).await,
        )),
        other => Err(other),
    }
}

fn typed<M: medulla_protocol_types::result_contract::MethodResult>(
    req_id: u64,
    result: Result<M::Body, String>,
) -> Response {
    match result.and_then(ResultPayload::of::<M>) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// A refusal as `CODE: detail`.
fn refuse(refusal: &ActionRefusal) -> String {
    let detail = serde_json::to_value(refusal)
        .ok()
        .and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    format!("{}: {detail}", refusal.code())
}

async fn handle_action_register(
    ctx: &HandleContext<'_>,
    spec: ActionSpec,
) -> Result<String, String> {
    spec.validate().map_err(|r| refuse(&r))?;
    let body = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), "ActionSpec".into());
    properties.insert("name".into(), spec.name.clone().into());
    properties.insert(SPEC_BODY.into(), body.into());
    let node = upsert_node(ctx, format!("{SPEC_PREFIX}{}", spec.name), properties);
    commit(ctx, vec![node]).await?;
    Ok(spec.name)
}

async fn handle_action_submit(
    ctx: &HandleContext<'_>,
    action: &str,
    params: BTreeMap<String, ParamValue>,
) -> Result<ActionRun, String> {
    let spec = load_spec(ctx, action)?;
    // No source is attached to an action graph yet: a `WriteBack` effect is
    // refused as unattached until the governed proposal path is wired.
    let run = spec.submit(params, &BTreeSet::new());
    let mut operations = applied_operations(ctx, &run)?;
    let (run_id, node) = run_node(ctx, &run)?;
    operations.push(node);
    commit(ctx, operations).await?;
    emit_outbox(ctx, &run_id).await;
    match &run.refusal {
        Some(refusal) => Err(refuse(refusal)),
        None => Ok(run),
    }
}

fn load_spec(ctx: &HandleContext<'_>, action: &str) -> Result<ActionSpec, String> {
    let not_registered = || format!("ACTION_NOT_REGISTERED: {action}");
    let raw = ctx
        .core
        .get_node_properties(&format!("{SPEC_PREFIX}{action}"))
        .ok_or_else(not_registered)?;
    let node = medulla_types::msgpack::decode_property_value(&raw)
        .map_err(|e| format!("ACTION_SPEC_INVALID: {e:?}"))?;
    let body = node
        .get(SPEC_BODY)
        .and_then(Value::as_str)
        .ok_or_else(not_registered)?;
    serde_json::from_str(body).map_err(|e| format!("ACTION_SPEC_INVALID: {e}"))
}

fn stamp(properties: &mut Map<String, Value>, ctx: &HandleContext<'_>) {
    properties.insert("tenant_id".into(), ctx.verified_context.tenant().into());
    properties.insert("_owner".into(), ctx.verified_context.agent_id().into());
}

fn upsert_node(ctx: &HandleContext<'_>, id: String, mut properties: Map<String, Value>) -> Value {
    stamp(&mut properties, ctx);
    json!({"op": "upsert_node", "id": id, "properties": properties})
}

/// The batch operations of every `Applied` effect, in declared order.
fn applied_operations(ctx: &HandleContext<'_>, run: &ActionRun) -> Result<Vec<Value>, String> {
    run.effects
        .iter()
        .filter(|e| e.disposition == EffectDisposition::Applied)
        .map(|e| effect_operation(ctx, e))
        .collect()
}

fn effect_operation(ctx: &HandleContext<'_>, effect: &ResolvedEffect) -> Result<Value, String> {
    let body: Value = serde_json::from_str(&effect.text)
        .map_err(|e| format!("ACTION_SPEC_INVALID: effect body is not JSON: {e}"))?;
    let field = |key: &str| body.get(key).cloned().unwrap_or(Value::Null);
    let text = |key: &str| field(key).as_str().unwrap_or_default().to_string();
    Ok(match &effect.kind {
        EffectKind::SetProperty => {
            let mut properties = Map::new();
            properties.insert(text("property"), field("value"));
            upsert_node(ctx, text("node"), properties)
        }
        EffectKind::CreateNode => {
            let mut properties = field("properties").as_object().cloned().unwrap_or_default();
            properties.insert("type".into(), field("label"));
            upsert_node(ctx, text("id"), properties)
        }
        EffectKind::AddEdge => {
            let mut properties = Map::from_iter([("relationship".to_string(), field("label"))]);
            stamp(&mut properties, ctx);
            json!({"op": "upsert_edge", "source": text("from"), "target": text("to"), "properties": properties})
        }
        EffectKind::WriteBack { .. } => return Err("ACTION_WRITE_BACK_UNATTACHED".to_string()),
    })
}

fn hex_digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The `ActionRun` node: verdicts, parameters and the effect receipts.
fn run_node(ctx: &HandleContext<'_>, run: &ActionRun) -> Result<(String, Value), String> {
    let body = serde_json::to_string(run).map_err(|e| e.to_string())?;
    let params_digest = hex_digest(&serde_json::to_string(&run.params).map_err(|e| e.to_string())?);
    let key = format!(
        "{}|{}|{params_digest}",
        run.action,
        ctx.verified_context.idempotency_key()
    );
    let run_id = format!("action-run:{}", &hex_digest(&key)[..32]);
    let mut properties = Map::new();
    properties.insert("type".into(), "ActionRun".into());
    properties.insert("action".into(), run.action.clone().into());
    properties.insert("run_id".into(), run_id.clone().into());
    properties.insert("status".into(), json!(run.status));
    properties.insert("params_digest".into(), params_digest.into());
    properties.insert(RUN_BODY.into(), body.into());
    debug_assert!(run.status == ActionStatus::Applied || run.refusal.is_some());
    Ok((run_id.clone(), upsert_node(ctx, run_id, properties)))
}

/// The outbox of one committed submit: the `ActionRun` node's change event,
/// carrying its committed post-image, then the `__action_completed` marker.
#[cfg(feature = "streaming")]
async fn emit_outbox(ctx: &HandleContext<'_>, run_id: &str) {
    use medulla_server_cdc::cdc::CdcStateExt;
    use medulla_types::wire::CdcKind;
    let state = ctx.state.read().await;
    let Some(hub) = state.cdc_hub() else { return };
    hub.emit(
        ctx.graph_name,
        CdcKind::UpdateNode,
        run_id.to_string(),
        String::new(),
        None,
        ctx.core.get_node_properties(run_id),
    );
    hub.emit(
        ctx.graph_name,
        CdcKind::UpdateNode,
        "__action_completed".to_string(),
        String::new(),
        None,
        None,
    );
}

#[cfg(not(feature = "streaming"))]
async fn emit_outbox(_ctx: &HandleContext<'_>, _run_id: &str) {}

/// Commit `operations` as ONE `BatchUpdate` through the request boundary.
async fn commit(ctx: &HandleContext<'_>, operations: Vec<Value>) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "action batch encoding failed".to_string())?;
    let response = medulla_server_api::api::reentry::dispatch_verified_request(
        ctx.state,
        Request {
            id: ctx.req_id,
            graph: ctx.graph_name.to_string(),
            auth_token: String::new(),
            agent_id: ctx.caller.map(str::to_owned),
            method: Method::BatchUpdate { operations_msgpack },
        },
        ctx.verified_context.clone(),
    )
    .await;
    response.refusal_text().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R700.3
    #[test]
    fn refusal_text_leads_with_the_registered_code() {
        let text = refuse(&ActionRefusal::CriterionFailed("amount_cap".into()));
        assert_eq!(text, "ACTION_CRITERION_FAILED: amount_cap");
    }
}
