//! Served workflow registry writes (MEDULLA-DECISION-ENGINE-R166.3, R166.4).
//!
//! `WorkflowRegistry::Register` is gateway-routed (`Route::WorkflowRegistry`): the
//! identity is validated by `WorkflowRegistry::register`, then the registration
//! commits as the one `:WorkflowRegistration` node upsert through the staged
//! gateway. A refused register writes nothing. Registering the stored record
//! again is a no-op that returns it; a different lifecycle state is an update of
//! the same node. `Read` is a graph read served by the catalog admin route in
//! `medulla-h-admin` and never reaches this module.

use super::*;
use medulla_protocol_types::result_contract::coordination::WorkflowRegistryRegister;
use medulla_work_types::orchestration::workflows::{
    WorkflowRegistrationEntry, WorkflowRegistry, WorkflowRegistryError, WorkflowRegistryOp,
};

/// The closed refusal codes of the workflow registry; each is a registered
/// `ServerErrorCode`.
pub const REFUSALS: &[&str] = &[
    "WORKFLOW_ID_MISSING",
    "WORKFLOW_VERSION_MISSING",
    "WORKFLOW_DIGEST_INVALID",
    "WORKFLOW_IDENTITY_UNKNOWN",
    "WORKFLOW_UNAUTHORIZED",
];

fn refuse(error: WorkflowRegistryError, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", error.code())
}

/// What an admitted `Register` does.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Admitted {
    /// The same record is already stored: return it, commit nothing.
    Same(WorkflowRegistrationEntry),
    /// Commit this node upsert, then return the entry.
    Write(Method, WorkflowRegistrationEntry),
}

/// Admit one registration against the stored node, writing nothing.
pub(super) fn admit_registration(
    core: &GraphCore,
    tenant_id: &str,
    tenant_scope: &str,
    registration: &WorkflowRegistrationEntry,
) -> Result<Admitted, String> {
    let identity = &registration.identity;
    WorkflowRegistry::default()
        .register(identity.clone())
        .map_err(|error| refuse(error, "invalid workflow identity"))?;
    if !registration.owner_scope.is_empty() && registration.owner_scope != tenant_scope {
        return Err(refuse(
            WorkflowRegistryError::Unauthorized,
            "owner_scope must be the verified tenant scope",
        ));
    }
    let entry = WorkflowRegistrationEntry {
        identity: identity.clone(),
        owner_scope: tenant_scope.to_string(),
        lifecycle: registration.lifecycle,
    };
    let node_id = WorkflowRegistrationEntry::node_id(identity);
    let stored = core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok());
    if let Some(stored) = stored
        .as_ref()
        .and_then(|props| WorkflowRegistrationEntry::from_node_properties(props))
    {
        if stored.owner_scope != tenant_scope {
            return Err(refuse(
                WorkflowRegistryError::Unauthorized,
                format!("{node_id} belongs to another scope"),
            ));
        }
        if stored == entry {
            return Ok(Admitted::Same(entry));
        }
    }
    let properties_msgpack = rmp_serde::to_vec_named(&entry.node_properties(tenant_id))
        .map_err(|error| format!("INVALID_ARGUMENT: workflow encoding failed: {error}"))?;
    Ok(Admitted::Write(
        Method::AddNode {
            node_id,
            properties_msgpack,
        },
        entry,
    ))
}

/// The `Route::WorkflowRegistry` entry: admit the registration, then commit it
/// as the `AddNode` upsert through the coalescable gateway.
pub(super) async fn handle_workflow_register(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::WorkflowRegistry { op } = method else {
        return None;
    };
    let WorkflowRegistryOp::Register { registration } = op.as_ref() else {
        return Some(Response::err(
            ctx.req_id,
            "INVALID_ARGUMENT: WorkflowRegistry read is served by the catalog admin route",
        ));
    };
    let (write, entry) =
        match admit_registration(ctx.core, tenant_id, ctx.tenant_scope, registration) {
            Ok(Admitted::Same(entry)) => {
                return Some(Response::ok(
                    ctx.req_id,
                    ResultPayload::of::<WorkflowRegistryRegister>(entry),
                ));
            }
            Ok(Admitted::Write(write, entry)) => (write, entry),
            Err(error) => return Some(Response::err(ctx.req_id, error)),
        };
    let plan = MutationPlan::for_method(&write);
    let owned = write.clone();
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            ResultPayload::of::<WorkflowRegistryRegister>(entry)
        })
        .await,
    )
}
