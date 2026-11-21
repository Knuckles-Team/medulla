use super::*;
use medulla_server_graph::dispatch::shared::timed_read;

/// Identity and access control: principal registration and read-back, policy
/// export, RBAC administration and the multisig-governed mutation admission
/// path. `RegisterIdentity` belongs HERE — the pre-domain cut left it in the
/// channel group.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_identity_and_access_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        // ── Zero-Trust Consensus ─────────────────────────────────────────
        method @ Method::RegisterIdentity { .. } => {
            dispatch_identity_and_access_methods_arm_0(ctx, method).await
        }
        // Identity read-back (CONCEPT:MEDULLA-KG.compute.feature): closes the `RegisterIdentity`
        // blind-upsert gap. `RegisterIdentity` REPLACES a principal's whole role set on
        // every call, so a caller that wants to add a role without dropping one already
        // granted by a prior admission pass must read the current set back first. Gated at
        // the SAME `security:admin` scope as `RegisterIdentity` (see `medulla_capabilities::policy`),
        // enforced by the admin-scope check above the method match, so no additional
        // authorization is done here.
        method @ Method::GetIdentity { .. } => {
            dispatch_identity_and_access_methods_arm_1(ctx, method).await
        }
        // WorkItem delegation record read (MEDULLA-IDENTITY-R701), gated `identity:read`.
        #[cfg(feature = "security")]
        Method::GetDelegationRecord { work_item_id } => {
            dispatch_get_delegation_record(state, req.id, verified_context, &work_item_id).await
        }

        // One graph's read-only ownership plan (MEDULLA-IDENTITY-R700.3), gated `identity:admin`.
        #[cfg(all(feature = "redb", feature = "security"))]
        Method::GraphOwnershipPlan { graph } => {
            medulla_h_admin::admin::ownership::handle_plan(state, req.id, &graph, verified_context)
                .await
        }

        // A principal's CURRENT access to the request graph, for an executor
        // acting on that principal's behalf (gated `security:check`).
        Method::CheckAccess { agent_id, access } => {
            dispatch_boxed(dispatch_check_access(ctx, agent_id, access)).await
        }

        // May an agent call one operation of a protocol resource now (MEDULLA-IDENTITY-R704.2),
        // gated `security:check`. The decision is a reply; a refusal carries its reason code.
        #[cfg(feature = "security")]
        Method::CheckResourceAccess {
            agent_id,
            resource_ref,
            protocol,
            operation,
        } => {
            dispatch_boxed(dispatch_check_resource_access(
                ctx,
                agent_id,
                resource_ref,
                protocol,
                operation,
            ))
            .await
        }

        // ── RBAC policy administration (CONCEPT:MEDULLA-KG.compute.feature) ──────────────────
        // Gated at the handler; a non-security build has no arm and falls to the
        // dispatch "not available in this build" catch-all (mirrors MEDULLA-090).
        #[cfg(feature = "security")]
        method @ Method::RbacAdmin { .. } => {
            dispatch_identity_and_access_methods_arm_2(ctx, method).await
        }

        method @ Method::ApplyMultisigMutation { .. } => {
            dispatch_identity_and_access_methods_arm_3(ctx, method).await
        }

        // ── Just-in-time RBAC elevation (EH-404) ──────────────────────────
        // Security-only like `RbacAdmin`: the ledger lives in the RBAC image.
        #[cfg(feature = "security")]
        Method::RbacElevation { op, actor } => {
            medulla_server_identity::dispatch::elevation::dispatch_rbac_elevation(
                state,
                req.id,
                verified_context,
                medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
                    state_machine_authorized,
                ),
                (op, actor),
            )
            .await
        }
        // ── Governed changes (EH-560) ─────────────────────────────────────
        #[cfg(feature = "security")]
        Method::GovernedChange { op, actor } => {
            medulla_server_identity::dispatch::governed::dispatch_governed_change(
                state,
                req.id,
                verified_context,
                medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
                    state_machine_authorized,
                ),
                (op, actor),
                medulla_server_identity::dispatch::governed::LeaseGraph {
                    graph: &req.graph,
                    caller: req.agent_id.as_deref(),
                },
            )
            .await
        }
        other => return dispatch_identity_store_method(ctx, other).await,
    })
}

/// Keep the identity-store route in this domain, including feature fallback.
async fn dispatch_identity_store_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    #[cfg(not(feature = "security"))]
    let _ = &ctx;
    match method {
        // MEDULLA-TYPED-PACKS-R652.3: the classification claim ops, served beside `Identity`.
        Method::ClassificationClaim { op } => ControlFlow::Break(
            medulla_server_identity::dispatch::classification::dispatch_classification(
                ctx.state,
                medulla_server_identity::dispatch::classification::ClassificationTarget {
                    graph: &ctx.req.graph,
                    req_id: ctx.req.id,
                    caller: ctx.req.agent_id.as_deref(),
                    verified: ctx.verified_context,
                },
                op,
            )
            .await,
        ),
        #[cfg(feature = "security")]
        Method::Identity { op, stamp } => ControlFlow::Break(
            medulla_server_identity::dispatch::identity_store::dispatch_identity(
                ctx.state,
                ctx.req.id,
                ctx.verified_context,
                medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
                    ctx.state_machine_authorized,
                ),
                (op, stamp),
            )
            .await,
        ),
        other => ControlFlow::Continue(other),
    }
}

/// `ApplyMultisigMutation` answers with the SPARQL UPDATE report of the
/// `ApplyMutation` it was translated into, declared under its own marker.
async fn dispatch_identity_and_access_methods_arm_0(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        Method::RegisterIdentity {
            agent_id,
            role,
            teams,
            signature,
            roles,
        } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_graph = req.graph.clone();
                {
                    let verification = if state_machine_authorized {
                        Ok(())
                    } else {
                        verify_register_identity_signature(
                            verified_context,
                            &req_graph,
                            &agent_id,
                            &role,
                            &teams,
                            &roles,
                            &signature,
                        )
                    };
                    if let Err(message) = verification {
                        medulla_kernel::metrics::process().auth_failure();
                        return Response::err(req_id, message);
                    }
                    let mut s = timed_write(state).await;
                    let identity = medulla_identity_types::acl::AgentIdentity {
                        agent_id: agent_id.clone(),
                        role,
                        teams,
                        roles,
                    };
                    if let Err(message) = register_identity(
                        &mut s,
                        identity,
                        IdentityRegistrationMode {
                            bootstrap: identity_bootstrap,
                            state_machine_authorized,
                            actor: verified_context.principal(),
                        },
                    ) {
                        return Response::err(req_id, message);
                    }
                    info!("RegisterIdentity committed");
                    Response::ok(
                        req_id,
                        ResultPayload::scalar::<
                            medulla_protocol_types::result_contract::security::RegisterIdentity,
                        >("registered".to_string()),
                    )
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

/// `CheckAccess`: the caller must itself read the request graph; the answer
/// is the engine's own admission decision for `agent_id` on that graph.
async fn dispatch_check_access(
    ctx: DispatchCtx<'_>,
    agent_id: String,
    access: medulla_identity_types::acl::AccessCheck,
) -> Response {
    let (req_id, graph) = (ctx.req.id, ctx.req.graph.clone());
    let s = timed_read(ctx.state).await;
    let Some(entry) = s.registry.get(&graph) else {
        return Response::err(req_id, format!("Graph '{graph}' not found"));
    };
    let caller = Some(ctx.verified_context.agent_id());
    let owner = entry.owner.as_deref();
    if let Err(denied) = medulla_server_auth::access::check_graph_access(
        &s.isolation,
        caller,
        &graph,
        entry.graph_type,
        owner,
        medulla_isolation::isolation::AccessLevel::Read,
    ) {
        return Response::err(req_id, denied);
    }
    let allowed = medulla_server_auth::access::principal_may_access(
        &s.isolation,
        &agent_id,
        &graph,
        entry.graph_type,
        owner,
        access,
    );
    drop(s);
    let decision = medulla_identity_types::acl::AccessDecision {
        agent_id,
        graph,
        access,
        allowed,
    };
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::security::CheckAccess>(
            decision,
        ),
    )
}

/// `CheckResourceAccess` (MEDULLA-IDENTITY-R704.2): the resource grant decision for one agent,
/// resource, protocol and operation, under one read of the isolation layer.
#[cfg(feature = "security")]
async fn dispatch_check_resource_access(
    ctx: DispatchCtx<'_>,
    agent_id: String,
    resource_ref: String,
    protocol: medulla_identity_types::acl::ResourceProtocol,
    operation: String,
) -> Response {
    let req_id = ctx.req.id;
    let now_ms = medulla_server_api::txn::now_ms();
    let s = timed_read(ctx.state).await;
    let decision = resource_access_decision(
        &s.isolation,
        &agent_id,
        &resource_ref,
        protocol,
        &operation,
        now_ms,
    );
    drop(s);
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::security::CheckResourceAccess>(
            decision,
        ),
    )
}

/// The decision itself: the subjects are the agent and every role of its registered identity
/// (an unregistered agent has none); no grant naming the resource is `RESOURCE_UNKNOWN`, none
/// matching a subject, protocol and operation is `RESOURCE_GRANT_MISSING`, every match ended
/// is `GRANT_EXPIRED`, otherwise the unexpired match with the latest expiry (ties by id)
/// allows.
#[cfg(feature = "security")]
fn resource_access_decision(
    isolation: &medulla_isolation::isolation::IsolationLayer,
    agent_id: &str,
    resource_ref: &str,
    protocol: medulla_identity_types::acl::ResourceProtocol,
    operation: &str,
    now_ms: u64,
) -> medulla_identity_types::acl::ResourceAccessDecision {
    use medulla_identity_types::acl::{ResourceAccessDecision, ResourceGrantSubject};
    use medulla_types::contract::ServerErrorCode;

    let revocation_revision = isolation.policy_revision();
    let refusal = |code: ServerErrorCode| ResourceAccessDecision {
        allow: false,
        grant_id: None,
        scope_ceiling: Vec::new(),
        expires_at_ms: None,
        revocation_revision,
        reason_code: Some(code.as_str().to_string()),
    };
    let ledger = isolation.rbac().resource_grants();
    if !ledger.names_resource(resource_ref) {
        return refusal(ServerErrorCode::ResourceUnknown);
    }
    let mut subjects = vec![ResourceGrantSubject::Principal(agent_id.to_string())];
    if let Some(identity) = isolation.get_identity(agent_id) {
        subjects.extend(identity.roles.into_iter().map(ResourceGrantSubject::Role));
    }
    let mut matching = ledger
        .matching(&subjects, resource_ref, protocol, operation)
        .peekable();
    if matching.peek().is_none() {
        return refusal(ServerErrorCode::ResourceGrantMissing);
    }
    let live = matching
        .filter(|grant| grant.expires_at_ms > now_ms)
        .max_by(|a, b| {
            a.expires_at_ms
                .cmp(&b.expires_at_ms)
                .then_with(|| b.grant_id.cmp(&a.grant_id))
        });
    match live {
        Some(grant) => ResourceAccessDecision {
            allow: true,
            grant_id: Some(grant.grant_id.clone()),
            scope_ceiling: grant.scope_ceiling.clone(),
            expires_at_ms: Some(grant.expires_at_ms),
            revocation_revision,
            reason_code: None,
        },
        None => refusal(ServerErrorCode::GrantExpired),
    }
}

struct IdentityRegistrationMode<'a> {
    bootstrap: bool,
    state_machine_authorized: bool,
    /// Who registered, for the identity store's own audit entry.
    actor: &'a str,
}

fn register_identity(
    state: &mut ServerState,
    identity: medulla_identity_types::acl::AgentIdentity,
    mode: IdentityRegistrationMode<'_>,
) -> Result<(), String> {
    if mode.bootstrap
        || (mode.state_machine_authorized && replicated_identity_bootstrap_authorized())
    {
        return state.isolation.try_bootstrap_system_identity(identity);
    }
    #[cfg(feature = "security")]
    {
        let actor = medulla_isolation::isolation::AuditActor {
            principal: mode.actor,
            now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            // The registration arm carries no verified tenant in scope yet;
            // the entry names one once it does (R002 follow-up).
            tenant: None,
        };
        let outcome = state.isolation.try_register_agent_audited(identity, actor);
        state.publish_identity_view();
        outcome
    }
    #[cfg(not(feature = "security"))]
    {
        let _ = mode.actor;
        state.isolation.try_register_agent_from_request(identity)
    }
}

async fn dispatch_identity_and_access_methods_arm_1(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        Method::GetIdentity { agent_id } => {
            dispatch_boxed(dispatch_get_identity(state, req.id, &req.graph, agent_id)).await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "security")]
async fn dispatch_identity_and_access_methods_arm_2(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        Method::RbacAdmin {
            op: op @ (medulla_identity_types::acl::RbacAdminOp::ShareGrant(_)
            | medulla_identity_types::acl::RbacAdminOp::ShareRevoke(_)),
        } => {
            // A share commits through the graph gateway of the request's graph
            // (MEDULLA-IDENTITY-R023.13.3), not through the RBAC image.
            dispatch_boxed(super::share::apply_share_op(
                state,
                req,
                verified_context,
                op,
            ))
            .await
        }
        Method::RbacAdmin { op } => {
            let actor = verified_context.principal();
            dispatch_boxed(apply_rbac_admin(state, req.id, op, actor)).await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

async fn dispatch_identity_and_access_methods_arm_3(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        Method::ApplyMultisigMutation {
            signatures,
            threshold,
            mutation_type,
            query,
        } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                let req_graph = req.graph.clone();
                {
                    if !state_machine_authorized {
                        if let Err(message) = verify_multisig_mutation_signatures(
                            verified_context,
                            &req_graph,
                            &signatures,
                            threshold,
                            &mutation_type,
                            &query,
                        ) {
                            medulla_kernel::metrics::process().auth_failure();
                            return Response::err(req_id, message);
                        }
                    }
                    // Delegate mutation application to the target graph
                    multisig_mutation_response(
                        dispatch_graph_op(
                            state,
                            &req_graph,
                            req_id,
                            req_agent_id.as_deref(),
                            verified_context,
                            Method::ApplyMutation {
                                event_type: mutation_type,
                                query,
                            },
                        )
                        .await,
                    )
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}
fn multisig_mutation_response(response: Response) -> Response {
    if let Some(refusal) = response.refusal_text() {
        return Response::err(response.id, refusal);
    }
    let Response { id, result, .. } = response;
    let report = match result {
        Some(ResultPayload::Json(value)) => serde_json::from_value::<
            medulla_protocol_types::result_contract::transactions::SparqlUpdateReport,
        >(value)
        .map_err(|error| {
            format!(
                "ApplyMultisigMutation: the translated ApplyMutation report is invalid: {error}"
            )
        }),
        _ => Err(
            "ApplyMultisigMutation: the translated ApplyMutation answered no report".to_string(),
        ),
    };
    Response::ok(
        id,
        report.and_then(
            ResultPayload::of::<
                medulla_protocol_types::result_contract::transactions::ApplyMultisigMutation,
            >,
        ),
    )
}

/// `GetDelegationRecord`: the stored delegation of one WorkItem under the verified
/// request tenant. A foreign-tenant record resolves exactly as an absent one.
#[cfg(feature = "security")]
async fn dispatch_get_delegation_record(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    context: &VerifiedRequestContext,
    work_item_id: &str,
) -> Response {
    let server = state.read().await.context.clone();
    let Some(tenant) =
        medulla_server_identity::dispatch::identity_store::resolution_context::record_tenant(
            &server, context,
        )
    else {
        return Response::err(
            req_id,
            "ACCESS_DENIED: the request carries no verified tenant",
        );
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let found = {
        let guard = timed_read(state).await;
        guard.isolation.rbac().identity_store().delegation_state(
            work_item_id,
            &tenant,
            now_ms,
            &medulla_capabilities::scopes::ScopeRegistry,
        )
    };
    match found {
        Some(delegation) => Response::ok(
            req_id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::security::GetDelegationRecord,
            >(delegation),
        ),
        None => Response::err(
            req_id,
            "DELEGATION_UNKNOWN: no delegation record for this work item",
        ),
    }
}
