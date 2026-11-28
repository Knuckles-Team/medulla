//! Served harness edit lifecycle (MEDULLA-DECISION-ENGINE-R173.7, R173.8).
//!
//! The engine owns the `core:editStatus` of a harness edit. A `Transition` is
//! gateway-routed (`Route::HarnessEdit`): the edit node is read, the event is applied with
//! `EditStatus::apply` (`EditMachine`), and the one `AddNode` upsert of the same properties
//! with only `editStatus` replaced commits through the staged gateway. The commit closure
//! re-reads the status and refuses `REVISION_CONFLICT` when it moved, so two racing events
//! never both apply. A refused transition writes nothing. A `Get` is a graph read.
//!
//! The tenant is the verified request tenant: the request carries no tenant field. An edit
//! that is absent, is not a harness edit, or belongs to another tenant answers the one
//! `HARNESS_EDIT_UNKNOWN` envelope, so a caller learns nothing about another tenant's edits.

use super::ops_context::GraphOpsContext;
use super::*;
use medulla_protocol_types::result_contract::coordination::{
    HarnessEditCreate, HarnessEditGet, HarnessEditTransition,
};
use medulla_types::edit_machine::{EditEvent, EditStatus, StatusWriter, EDIT_MACHINE};
use medulla_types::transition_refused::MachineState;
use medulla_work_types::harness_edit::{
    HarnessEditOp, HarnessEditRecord, HARNESS_EDIT_EXISTS, HARNESS_EDIT_STATUS_INVALID,
    HARNESS_EDIT_UNKNOWN,
};

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// The one non-disclosing envelope of an absent, foreign or non-edit node.
fn unknown(edit_id: &str) -> String {
    refuse(
        HARNESS_EDIT_UNKNOWN.as_str(),
        format!("no harness edit {edit_id} is available to the caller's tenant"),
    )
}

/// The stored properties of the harness edit `edit_id` that `tenant` may see. A node that
/// carries no tenant belongs to the graph, like the concentration question reads it.
fn stored_properties(
    core: &GraphCore,
    tenant: &str,
    edit_id: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let blob = core
        .get_node_properties(edit_id)
        .ok_or_else(|| unknown(edit_id))?;
    let properties =
        medulla_types::msgpack::decode_property_object(&blob).map_err(|_| unknown(edit_id))?;
    if properties.get("_harness_edit") != Some(&serde_json::Value::Bool(true)) {
        return Err(unknown(edit_id));
    }
    let foreign = properties
        .get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|owner| owner != tenant);
    if foreign {
        return Err(unknown(edit_id));
    }
    Ok(properties)
}

/// The status the stored literal names, or why it names none.
fn status_of(
    properties: &serde_json::Map<String, serde_json::Value>,
) -> Result<EditStatus, String> {
    let word = properties
        .get("editStatus")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    EditStatus::from_stored(word).ok_or_else(|| {
        refuse(
            HARNESS_EDIT_STATUS_INVALID.as_str(),
            format!("the stored status {word:?} is not a status of EditMachine"),
        )
    })
}

/// Answer one get: the recorded status of the caller's edit.
pub fn read_op(core: &GraphCore, tenant: &str, edit_id: &str) -> Result<HarnessEditRecord, String> {
    let properties = stored_properties(core, tenant, edit_id)?;
    Ok(HarnessEditRecord {
        edit_id: edit_id.to_string(),
        status: status_of(&properties)?,
    })
}

/// An admitted transition: the upsert to commit, the record it stores and the status the
/// event was applied to.
pub struct Admitted {
    pub write: Method,
    pub record: HarnessEditRecord,
    pub from: EditStatus,
}

/// Admit one event against the stored edit, writing nothing.
pub fn admit_transition(
    core: &GraphCore,
    tenant: &str,
    edit_id: &str,
    event: EditEvent,
) -> Result<Admitted, String> {
    let mut properties = stored_properties(core, tenant, edit_id)?;
    let from = status_of(&properties)?;
    let to = from.apply(event).map_err(|refused| refused.to_string())?;
    properties.insert(
        "editStatus".to_string(),
        serde_json::Value::String(to.stored().to_string()),
    );
    let properties_msgpack = rmp_serde::to_vec_named(&serde_json::Value::Object(properties))
        .map_err(|error| {
            refuse(
                "PARAMETER_INVALID",
                format!("edit encoding failed: {error}"),
            )
        })?;
    Ok(Admitted {
        write: Method::AddNode {
            node_id: edit_id.to_string(),
            properties_msgpack,
        },
        record: HarnessEditRecord {
            edit_id: edit_id.to_string(),
            status: to,
        },
        from,
    })
}

/// The edit node and `targetsDimension` edge of one `Create`, as the one `BatchUpdate` that
/// commits both. The initial status is the initial state of `EditMachine`, never the
/// caller's.
pub fn create_batch(
    tenant: &str,
    edit_id: &str,
    dimension_iri: &str,
    round: u32,
    policy_revision: Option<u64>,
) -> Result<(Method, Vec<u8>), String> {
    let mut properties = serde_json::json!({
        "_harness_edit": true,
        "editStatus": EDIT_MACHINE.initial.stored(),
        "editRound": round,
        "tenant_id": tenant,
    });
    if let Some(revision) = policy_revision {
        properties["policyRevision"] = serde_json::json!(revision);
    }
    let operations = serde_json::json!([
        {"op": "upsert_node", "id": edit_id, "properties": properties},
        {
            "op": "upsert_edge",
            "source": edit_id,
            "target": dimension_iri,
            "properties": {"relationship": "targetsDimension"},
        },
    ]);
    let operations_msgpack = rmp_serde::to_vec_named(&operations).map_err(|error| {
        refuse(
            "PARAMETER_INVALID",
            format!("edit encoding failed: {error}"),
        )
    })?;
    Ok((
        Method::BatchUpdate {
            operations_msgpack: operations_msgpack.clone(),
        },
        operations_msgpack,
    ))
}

/// Serve one `Create`: the edit node and its `targetsDimension` edge commit in one batch
/// through the staged gateway, under the edit machine as the writer.
async fn handle_create(
    ctx: &MutationCtx<'_>,
    edit_id: &str,
    dimension_iri: &str,
    round: u32,
    policy_revision: Option<u64>,
) -> Response {
    if ctx.core.get_node_properties(edit_id).is_some() {
        return Response::err(
            ctx.req_id,
            refuse(
                HARNESS_EDIT_EXISTS.as_str(),
                format!("harness edit {edit_id} already exists"),
            ),
        );
    }
    if ctx.core.get_node_properties(dimension_iri).is_none() {
        return Response::err(
            ctx.req_id,
            refuse(
                "PARAMETER_INVALID",
                format!("dimension {dimension_iri} is not a node of the graph"),
            ),
        );
    }
    let (write, operations) = match create_batch(
        ctx.tenant_scope,
        edit_id,
        dimension_iri,
        round,
        policy_revision,
    ) {
        Ok(batch) => batch,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let record = HarnessEditRecord {
        edit_id: edit_id.to_string(),
        status: EDIT_MACHINE.initial,
    };
    let plan = MutationPlan::for_method(&write);
    let edit_id = edit_id.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        if core.get_node_properties(&edit_id).is_some() {
            return Err(refuse(
                HARNESS_EDIT_EXISTS.as_str(),
                format!("harness edit {edit_id} already exists"),
            ));
        }
        medulla_graph_algos::algorithms::batch_update(core, &operations)?;
        ResultPayload::of::<HarnessEditCreate>(record)
    })
    .await
}

/// The `Route::HarnessEdit` entry: a `Create` commits the edit node and edge, a `Transition`
/// admits the event and commits the node upsert through the coalescable gateway. Both commit
/// under [`StatusWriter::EditMachine`], the one writer of `editStatus` the mutation compiler
/// admits.
pub async fn handle_mutation(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::HarnessEdit { op } = method else {
        return None;
    };
    if let Err(detail) = op.check() {
        return Some(Response::err(
            ctx.req_id,
            refuse("PARAMETER_INVALID", detail),
        ));
    }
    let ctx = &MutationCtx {
        status_writer: StatusWriter::EditMachine,
        ..*ctx
    };
    let (edit_id, event) = match op.as_ref() {
        HarnessEditOp::Create {
            edit_id,
            dimension_iri,
            round,
            policy_revision,
        } => {
            return Some(
                handle_create(ctx, edit_id, dimension_iri, *round, *policy_revision).await,
            );
        }
        HarnessEditOp::Transition { edit_id, event } => (edit_id, event),
        HarnessEditOp::Get { .. } => {
            return Some(Response::err(
                ctx.req_id,
                refuse(
                    "PARAMETER_INVALID",
                    "a HarnessEdit get is served by the graph read path",
                ),
            ));
        }
    };
    let tenant = ctx.tenant_scope;
    let admitted = match admit_transition(ctx.core, tenant, edit_id, *event) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let Admitted {
        write,
        record,
        from,
    } = admitted;
    let plan = MutationPlan::for_method(&write);
    let (tenant, edit_id, owned) = (tenant.to_string(), edit_id.clone(), write.clone());
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            let current = status_of(&stored_properties(core, &tenant, &edit_id)?)?;
            if current != from {
                return Err(refuse(
                    "REVISION_CONFLICT",
                    format!(
                        "harness edit {edit_id} moved to {} while its event was applied to {}",
                        current.stored(),
                        from.stored()
                    ),
                ));
            }
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            ResultPayload::of::<HarnessEditTransition>(record)
        })
        .await,
    )
}

/// Route the harness edit reads served from the graph.
pub async fn try_handle_harness_edit(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::HarnessEdit { op } = &method else {
        return ControlFlow::Continue(method);
    };
    let HarnessEditOp::Get { edit_id } = op.as_ref() else {
        // A transition is gateway-routed and never reaches the graph read path.
        return ControlFlow::Continue(method);
    };
    if let Err(detail) = op.check() {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse("PARAMETER_INVALID", detail),
        ));
    }
    let Some(tenant) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "a harness edit read needs a verified tenant",
            ),
        ));
    };
    ControlFlow::Break(match read_op(ctx.raw_core, &tenant, edit_id) {
        Ok(record) => Response::ok(ctx.req_id, ResultPayload::of::<HarnessEditGet>(record)),
        Err(error) => Response::err(ctx.req_id, error),
    })
}
