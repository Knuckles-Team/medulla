//! Served data products, contracts and access requests (MEDULLA-TYPED-PACKS-R805.3).
//!
//! `DataProductRegister`, `AccessRequestOpen` and `AccessRequestDecide` are decided here
//! against the durable policy store (`rbac_data_product` in `rbac.redb`), keyed by the
//! verified tenant. Approving a request writes its grant through the one sharing enum: for
//! each dataset of the product the node's share is decided by `share_transition` with the
//! principal added to the grantees, and committed through the same audited path as a served
//! `ShareGrant` (`commit_share`). The approver is never a forged admin: the verified caller
//! must be the owner of the product or an owner or steward of one of its datasets by the
//! stewardship store, and `share_transition` accepts that role as a `ShareDelegation` whose
//! reason the audit entry records. Every refusal is decided before the first write.

use super::share::{
    commit_share, node_absent, read_node, record_share_event, state_of, visible_to, Properties,
    ShareEvent, SharePhase, ADMIN_SCOPE,
};
use super::*;
use medulla_catalog_types::data_product::{
    AccessRequest, AccessRequestState, DataProductRegistered,
};
use medulla_core::stewardship::{reviewers, TargetScopes};
use medulla_identity_types::acl::ShareGrant;
use medulla_identity_types::identity::ShareScope;
use medulla_isolation::isolation::share::{
    share_authority, share_transition, ShareActor, ShareDelegation, ShareOp, ShareState,
    SCOPE_COMMONS, SCOPE_ORG,
};
use medulla_protocol_types::result_contract::storage::{
    AccessRequestDecide, AccessRequestOpen, DataProductRegister,
};
use medulla_server_graph::dispatch::graph_client::GraphTarget;
use medulla_server_graph::dispatch::shared::typed_response;
use medulla_types::access_request_machine::AccessRequestEvent;
use medulla_types::contract::ServerErrorCode;

/// Serve one data product method; hands any other method back.
pub async fn dispatch_data_product_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        Method::DataProductRegister {
            tenant,
            product,
            contract,
        } => {
            dispatch_boxed(async {
                typed_response::<DataProductRegister>(
                    req.id,
                    register(state, verified_context, &tenant, product, contract).await,
                )
            })
            .await
        }
        Method::AccessRequestOpen { tenant, request } => {
            dispatch_boxed(async {
                typed_response::<AccessRequestOpen>(
                    req.id,
                    open(state, verified_context, &tenant, request).await,
                )
            })
            .await
        }
        Method::AccessRequestDecide {
            tenant,
            product,
            principal,
            approve,
        } => {
            dispatch_boxed(async {
                typed_response::<AccessRequestDecide>(
                    req.id,
                    decide(
                        state,
                        req,
                        verified_context,
                        &tenant,
                        &product,
                        &principal,
                        approve,
                    )
                    .await,
                )
            })
            .await
        }
        other => return ControlFlow::Continue(other),
    })
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

/// The `tenant` of a method must be the verified request tenant.
fn require_tenant(verified: &VerifiedRequestContext, tenant: &str) -> Result<(), String> {
    if verified.tenant().trim().is_empty() {
        return Err(ServerErrorCode::TenantScopeRequired.as_str().to_string());
    }
    if verified.tenant() != tenant {
        return Err(format!(
            "{}: the tenant of the method is not the verified tenant",
            ServerErrorCode::RecordTenantMismatch.as_str()
        ));
    }
    Ok(())
}

async fn policy_store(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Arc<dyn medulla_isolation::rbac_persist::RbacPolicyStore>, String> {
    state
        .read()
        .await
        .isolation
        .policy_store()
        .ok_or_else(|| invalid("data products require the durable policy store"))
}

fn store_error(error: medulla_isolation::rbac_persist::RbacPersistError) -> String {
    error.to_string()
}

async fn register(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    tenant: &str,
    product: medulla_catalog_types::data_product::DataProduct,
    contract: medulla_catalog_types::data_product::DataContract,
) -> Result<DataProductRegistered, String> {
    require_tenant(verified, tenant)?;
    product.validate().map_err(invalid)?;
    contract.validate().map_err(invalid)?;
    if contract.product != product.id {
        return Err(invalid("the contract names another product"));
    }
    // The owner of a product registers it; a kg:admin holder registers on any owner's behalf.
    if product.owner != verified.agent_id() && !verified.allows_action(ADMIN_SCOPE) {
        return Err(format!(
            "{}: only the product owner or a kg:admin holder registers a product",
            ServerErrorCode::StewardRoleMissing.as_str()
        ));
    }
    let store = policy_store(state).await?;
    store
        .put_data_product(tenant, &product, &contract)
        .map_err(store_error)?;
    Ok(DataProductRegistered {
        product: product.id,
        version: product.version,
    })
}

async fn open(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    tenant: &str,
    request: AccessRequest,
) -> Result<AccessRequest, String> {
    require_tenant(verified, tenant)?;
    request.validate().map_err(invalid)?;
    if request.state != AccessRequestState::Pending || request.grant_ref.is_some() {
        return Err(invalid("a new access request is pending and holds no grant"));
    }
    // A request is the caller's own; nobody opens one for another principal.
    if request.principal != verified.agent_id() {
        return Err(format!(
            "{}: an access request is opened by the principal it names",
            ServerErrorCode::StewardRoleMissing.as_str()
        ));
    }
    let store = policy_store(state).await?;
    if store
        .data_product(tenant, &request.product)
        .map_err(store_error)?
        .is_none()
    {
        return Err(invalid(format!(
            "data product '{}' is not registered",
            request.product
        )));
    }
    if let Some(held) = store
        .access_request(tenant, &request.product, &request.principal)
        .map_err(store_error)?
    {
        if matches!(
            held.state,
            AccessRequestState::Pending | AccessRequestState::Approved
        ) {
            return Err(invalid(format!(
                "an access request of '{}' for '{}' is already {}",
                request.principal,
                request.product,
                medulla_types::transition_refused::MachineState::stored(held.state)
            )));
        }
    }
    store
        .put_access_request(tenant, &request)
        .map_err(store_error)?;
    Ok(request)
}

/// The role under which the verified caller decides requests of `product`: its owner, or an
/// owner or steward of one of its datasets by the stored assignments.
async fn decider_role(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    product: &medulla_catalog_types::data_product::DataProduct,
) -> Result<ShareDelegation, String> {
    let caller = verified.agent_id();
    if product.owner == caller {
        return Ok(ShareDelegation::ProductOwner {
            product: product.id.clone(),
        });
    }
    let target = GraphTarget {
        graph: &req.graph,
        req_id: req.id,
        caller: req.agent_id.as_deref(),
        verified,
    };
    let assignments =
        medulla_server_graph::dispatch::stewardship::load_assignments(state, &target).await?;
    let governs = product.datasets.iter().any(|dataset| {
        let scopes = TargetScopes {
            asset: Some(dataset.clone()),
            concept_scheme: None,
            domain: None,
        };
        reviewers(&assignments, &scopes)
            .iter()
            .any(|reviewer| reviewer == caller)
    });
    if governs {
        Ok(ShareDelegation::ProductSteward {
            product: product.id.clone(),
        })
    } else {
        Err(format!(
            "{}: the caller is neither the product owner nor a steward of its datasets",
            ServerErrorCode::StewardRoleMissing.as_str()
        ))
    }
}

/// One dataset's decided share: the node's committed keys and the state the grant leaves.
struct DatasetShare {
    node_id: String,
    properties: Properties,
    current: ShareState,
    next: ShareState,
    reason: String,
}

/// Decide, without writing, the share that lets `principal` read `dataset`. `None` when the
/// principal can already read it: the node is shared with the whole tenant or the commons, or
/// names the principal.
async fn plan_dataset_share(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    dataset: &str,
    principal: &str,
    delegation: &ShareDelegation,
) -> Result<Option<DatasetShare>, String> {
    let properties = match read_node(state, req, verified, &req.graph, dataset).await {
        Ok(Some(properties)) => properties,
        Ok(None) => return Err(node_absent(dataset)),
        Err(refusal) if refusal.starts_with("UNIT_OUT_OF_SCOPE") => {
            return Err(node_absent(dataset))
        }
        Err(refusal) => return Err(refusal),
    };
    let current = state_of(dataset, &properties);
    let open_to_tenant = current.shared_scope == SCOPE_COMMONS
        || (current.shared_scope == SCOPE_ORG && current.grants.is_empty());
    if open_to_tenant || current.grants.iter().any(|grantee| grantee == principal) {
        return Ok(None);
    }
    let actor = ShareActor {
        principal_id: verified.principal().to_string(),
        admin: verified.allows_action(ADMIN_SCOPE),
        delegation: Some(delegation.clone()),
    };
    if !visible_to(&current, &actor) {
        return Err(node_absent(dataset));
    }
    let mut grantees = current.grants.clone();
    grantees.push(principal.to_string());
    grantees.sort();
    grantees.dedup();
    let grant = ShareGrant::new(dataset.to_string(), ShareScope::Tenant, grantees)
        .map_err(|refusal| invalid(format!("{refusal:?}")))?;
    let next = share_transition(&current, &ShareOp::Grant(grant), &actor)
        .map_err(|refusal| refusal.to_string())?;
    let reason = share_authority(&current, &actor)
        .map(|authority| authority.reason())
        .unwrap_or_default();
    Ok(Some(DatasetShare {
        node_id: dataset.to_string(),
        properties,
        current,
        next,
        reason,
    }))
}

/// Commit one planned share under its own reserved and outcome audit entries, which name the
/// authority the decider acted under.
async fn commit_dataset_share(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    share: &DatasetShare,
) -> Result<(), String> {
    let event = ShareEvent::new(
        "AccessRequestDecide.ShareGrant",
        &share.node_id,
        &share.current.shared_scope,
        &share.next.shared_scope,
        &share.next.grants,
    )
    .with_authority(share.reason.clone());
    record_share_event(state, req, verified, &event, SharePhase::Reserved).await?;
    match commit_share(
        state,
        req,
        verified,
        &share.node_id,
        &share.properties,
        &share.current,
        &share.next,
    )
    .await
    {
        Ok(()) => {
            record_share_event(state, req, verified, &event, SharePhase::Ok).await?;
            Ok(())
        }
        Err(refusal) => {
            let _ = record_share_event(state, req, verified, &event, SharePhase::Error).await;
            Err(refusal)
        }
    }
}

async fn decide(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    tenant: &str,
    product_id: &str,
    principal: &str,
    approve: bool,
) -> Result<AccessRequest, String> {
    require_tenant(verified, tenant)?;
    let store = policy_store(state).await?;
    let (product, _) = store
        .data_product(tenant, product_id)
        .map_err(store_error)?
        .ok_or_else(|| invalid(format!("data product '{product_id}' is not registered")))?;
    let mut request = store
        .access_request(tenant, product_id, principal)
        .map_err(store_error)?
        .ok_or_else(|| {
            invalid(format!(
                "no access request of '{principal}' for '{product_id}'"
            ))
        })?;
    let delegation = decider_role(state, req, verified, &product).await?;
    let event = if approve {
        AccessRequestEvent::Approve
    } else {
        AccessRequestEvent::Reject
    };
    let next_state = request.state.apply(event).map_err(|refusal| refusal.to_string())?;
    if approve {
        // Every dataset's share is decided before the first one is written, so a refused
        // dataset leaves every node as it was. A share a retry finds already written is
        // skipped, which makes an approval that failed at the store write repeatable.
        let mut planned = Vec::with_capacity(product.datasets.len());
        for dataset in &product.datasets {
            if let Some(share) =
                plan_dataset_share(state, req, verified, dataset, principal, &delegation).await?
            {
                planned.push(share);
            }
        }
        for share in &planned {
            commit_dataset_share(state, req, verified, share).await?;
        }
        request.grant_ref = Some(format!("share:{}:{}", product.id, principal));
    }
    request.state = next_state;
    store
        .put_access_request(tenant, &request)
        .map_err(store_error)?;
    Ok(request)
}
