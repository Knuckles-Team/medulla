//! Served run-bound observation state (MEDULLA-DECISION-ENGINE-R816).
//!
//! `ObservationState` puts are gateway-routed (`Route::ObservationState`): the
//! record is admitted against the stored node, then commits as the one
//! `AddNode` upsert through the staged gateway, so it is durable and
//! replicated like every other graph write. A refused put writes nothing.
//! A get is a graph read over the raw tenant graph.
//!
//! The record key is `(tenant, run_id, source, frame, revision)` and the
//! tenant is the verified request tenant: the request carries no tenant
//! field. A read of a key the caller's tenant never stored, including one
//! another tenant stored, answers with the one `VISUAL_STATE_CROSS_TENANT`
//! envelope, so it does not disclose whether another tenant holds it. A
//! stored record read after its `expires_at_ms` answers `VISUAL_STATE_STALE`.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::coordination::{
    ObservationStateGet, ObservationStatePut,
};
use medulla_work_types::observation_state::{
    ObservationInput, ObservationKey, ObservationRecord, ObservationStateOp,
};

const LABEL: &str = "ObservationState";

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The one non-disclosing envelope of an absent or foreign observation.
fn unavailable(key: &ObservationKey) -> String {
    refuse(
        "VISUAL_STATE_CROSS_TENANT",
        format!(
            "no observation state of run {} source {} frame {} revision {} is available to the caller's tenant",
            key.run_id, key.source, key.frame, key.revision
        ),
    )
}

/// The stored record of `key` in `tenant`; anything else reads as absent.
pub(super) fn read_stored(
    core: &GraphCore,
    tenant: &str,
    key: &ObservationKey,
) -> Option<ObservationRecord> {
    let blob = core.get_node_properties(&key.node_id(tenant))?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    if props.get("type").and_then(|value| value.as_str()) != Some(LABEL)
        || props.get("tenant_scope").and_then(|value| value.as_str()) != Some(tenant)
    {
        return None;
    }
    let record: ObservationRecord = serde_json::from_str(props.get("json")?.as_str()?).ok()?;
    (record.tenant_id == tenant).then_some(record)
}

fn node_blob(tenant: &str, record: &ObservationRecord) -> Result<Vec<u8>, String> {
    let json = serde_json::to_string(record).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("record encoding failed: {error}"),
        )
    })?;
    rmp_serde::to_vec_named(&serde_json::json!({
        "type": LABEL,
        "tenant_scope": tenant,
        "json": json,
    }))
    .map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("record encoding failed: {error}"),
        )
    })
}

/// What an admitted put does.
pub(super) enum Admitted {
    /// The upsert to commit and the record it stores.
    Write(Box<Method>, ObservationRecord),
    /// The very same record is already stored: nothing to commit.
    Same(ObservationRecord),
}

/// Admit one put against the stored node, writing nothing.
pub(super) fn admit_put(
    core: &GraphCore,
    tenant: &str,
    now_ms: u64,
    input: &ObservationInput,
) -> Result<Admitted, String> {
    input
        .check(now_ms)
        .map_err(|detail| refuse("PARAMETER_INVALID", detail))?;
    let key = input.key();
    let record = input.clone().into_record(tenant);
    match read_stored(core, tenant, &key) {
        Some(stored) if stored == record => Ok(Admitted::Same(stored)),
        Some(_) => Err(refuse(
            "PARAMETER_INVALID",
            format!(
                "revision {} of run {} source {} frame {} is already stored with a different record",
                key.revision, key.run_id, key.source, key.frame
            ),
        )),
        None => {
            let write = Method::AddNode {
                node_id: key.node_id(tenant),
                properties_msgpack: node_blob(tenant, &record)?,
            };
            Ok(Admitted::Write(Box::new(write), record))
        }
    }
}

/// Answer one get: the live record of the caller's tenant.
pub(super) fn read_op(
    core: &GraphCore,
    tenant: &str,
    now_ms: u64,
    key: &ObservationKey,
) -> Result<ObservationRecord, String> {
    key.check()
        .map_err(|detail| refuse("PARAMETER_INVALID", detail))?;
    let record = read_stored(core, tenant, key).ok_or_else(|| unavailable(key))?;
    if !record.is_live(now_ms) {
        return Err(refuse(
            "VISUAL_STATE_STALE",
            format!(
                "observation state expired at {} and the authoritative clock is {now_ms}",
                record.expires_at_ms
            ),
        ));
    }
    Ok(record)
}

/// The `Route::ObservationState` entry: admit the put, then commit the node
/// upsert through the coalescable gateway.
pub(super) async fn handle_put(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ObservationState { op } = method else {
        return None;
    };
    let ObservationStateOp::Put { record } = op.as_ref() else {
        return Some(Response::err(
            ctx.req_id,
            refuse(
                "PARAMETER_INVALID",
                "an ObservationState get is served by the graph read path",
            ),
        ));
    };
    let tenant = ctx.tenant_scope;
    match admit_put(ctx.core, tenant, authoritative_now_ms(), record) {
        Err(error) => Some(Response::err(ctx.req_id, error)),
        Ok(Admitted::Same(stored)) => Some(Response::ok(
            ctx.req_id,
            ResultPayload::of::<ObservationStatePut>(stored),
        )),
        Ok(Admitted::Write(write, stored)) => {
            let plan = MutationPlan::for_method(&write);
            let owned = write.as_ref().clone();
            Some(
                super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
                    medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
                    ResultPayload::of::<ObservationStatePut>(stored)
                })
                .await,
            )
        }
    }
}

/// Route the observation reads served from the graph.
pub(super) async fn try_handle_observation_state(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ObservationState { op } = &method else {
        return ControlFlow::Continue(method);
    };
    let ObservationStateOp::Get { key } = op.as_ref() else {
        // A put is gateway-routed and never reaches the graph read path.
        return ControlFlow::Continue(method);
    };
    let Some(tenant) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "an observation state read needs a verified tenant",
            ),
        ));
    };
    ControlFlow::Break(
        match read_op(ctx.raw_core, &tenant, authoritative_now_ms(), key) {
            Ok(record) => {
                Response::ok(ctx.req_id, ResultPayload::of::<ObservationStateGet>(record))
            }
            Err(error) => Response::err(ctx.req_id, error),
        },
    )
}
