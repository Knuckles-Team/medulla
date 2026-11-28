use super::router::dispatch_request_method;
use crate::dispatch::*;
use medulla_server_auth::auth::RequestAuthorityExt;
use medulla_server_graph::dispatch::shared::timed_read;
#[cfg(feature = "redb")]
use medulla_server_router_arms::dispatch::request_boundary::saga::replayed_response;
use medulla_server_router_arms::dispatch::request_boundary::*;

/// Dispatch a single request to the appropriate handler, recording
/// per-operation request counters and latency (CONCEPT:MEDULLA-KG.txn.per-graph-write-isolation).
pub async fn dispatch(state: &Arc<RwLock<ServerState>>, req: Request) -> Response {
    dispatch_with_context(state, req, None).await
}

/// Dispatch a native transport request whose current envelope was verified
/// before optional QoS admission. This keeps authentication single-pass: a
/// mutation nonce is consumed by the durable MutationBatch ledger and a
/// read-only nonce by the transport replay ledger, while admission plus
/// dispatch share the same immutable verified context.
pub async fn dispatch_verified_request(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    context: VerifiedRequestContext,
) -> Response {
    dispatch_with_context(state, req, Some(context)).await
}

/// Bridge an already-authenticated auxiliary broker protocol into the same
/// authorization and dispatch path as the primary request transport. The
/// caller supplies only a secret-keyed opaque actor reference; raw protocol
/// usernames are never admitted to request or persistence state.
pub async fn dispatch_authenticated_broker_actor(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    actor_ref: &str,
) -> Response {
    let request_id = req.id;
    let server = state.read().await.context.clone();
    let context =
        match VerifiedRequestContext::authenticated_broker_actor(&server, actor_ref, request_id) {
            Ok(context) => context,
            Err(error) => {
                medulla_kernel::metrics::process().auth_failure();
                return method_errors::enforce(
                    (&req.method).into(),
                    Response::err(request_id, error),
                );
            }
        };
    dispatch_with_context(state, req, Some(context)).await
}

/// Bridge an authenticated RESP session into the served dispatch. The listener
/// supplies the secret-keyed actor reference of the principal its `AUTH` verified;
/// the context it receives carries the key-value grant only, so every command
/// passes the same admission and policy path as a signed request.
#[cfg(feature = "redis-wire")]
pub async fn dispatch_authenticated_resp_actor(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    actor_ref: &str,
) -> Response {
    let request_id = req.id;
    let server = state.read().await.context.clone();
    let context =
        match VerifiedRequestContext::authenticated_resp_actor(&server, actor_ref, request_id) {
            Ok(context) => context,
            Err(error) => {
                medulla_kernel::metrics::process().auth_failure();
                return method_errors::enforce(
                    (&req.method).into(),
                    Response::err(request_id, error),
                );
            }
        };
    dispatch_with_context(state, req, Some(context)).await
}

/// Dispatch an engine-owned local query adapter under its fixed, read-only
/// service identity. The caller remains subject to provisioned graph ACL/RBAC.
pub async fn dispatch_authenticated_local_query(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
) -> Response {
    let server = state.read().await.context.clone();
    let context = match VerifiedRequestContext::authenticated_local_query(&server, req.id) {
        Ok(context) => context,
        Err(error) => {
            return method_errors::enforce((&req.method).into(), Response::err(req.id, error));
        }
    };
    dispatch_with_context(state, req, Some(context)).await
}

pub async fn dispatch_with_context(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    context: Option<VerifiedRequestContext>,
) -> Response {
    // MEDULLA-OBSERVE-R006.2: the caller's trace id is recorded verbatim, so it is bounded.
    if let Some(trace_id) = req.trace_id.as_deref() {
        if trace_id.is_empty()
            || trace_id.len() > medulla_protocol_types::protocol::TRACE_ID_MAX_LEN
        {
            return method_errors::enforce(
                (&req.method).into(),
                Response::err(
                    req.id,
                    format!(
                        "INVALID_PARAMS: trace_id must be 1 to {} characters",
                        medulla_protocol_types::protocol::TRACE_ID_MAX_LEN
                    ),
                ),
            );
        }
    }
    // CONCEPT:MEDULLA-OS.observability.slow-query-descriptor — slow-query descriptor, captured BEFORE the method is moved
    // into `dispatch_inner`. `None` (zero cost) unless MEDULLA_SLOW_QUERY_MS
    // enabled it AND this is a query method.
    // MEDULLA-DURABLE-KERNEL-R101.3: every served request is foreground activity.
    medulla_server_api::downtime::FOREGROUND_ACTIVITY.note_request(
        medulla_server_api::txn::now_ms(),
        medulla_server_api::knob_config::knobs().downtime_idle_max_rps,
    );
    let method: &'static str = (&req.method).into();
    let slow = medulla_kernel::slow_query::describe(&req.method);
    #[cfg(feature = "metrics")]
    let op: &'static str = method;

    // Time the request when EITHER Prometheus metrics OR slow-query logging needs
    // it. When both are off (metrics feature disabled AND the threshold unset) we
    // skip the clock entirely — byte-for-byte the prior `not(metrics)` path.
    let start = (cfg!(feature = "metrics") || slow.is_some()).then(std::time::Instant::now);

    // MEDULLA-OBSERVE-R006.2: the request is listed from entry to every exit; its phase
    // follows `RequestMachine`. The guard removes the row when this future ends or is dropped.
    let principal = context
        .as_ref()
        .map(|context| context.agent_id().to_string())
        .unwrap_or_default();
    let request_guard =
        medulla_observe::requests().enter(medulla_observe::registry::requests::EnterRequest {
            request_id: req.id,
            trace_id: req.trace_id.as_deref(),
            method,
            graph: &req.graph,
            principal: &principal,
        });
    // MEDULLA-OBSERVE-R002.3: the one span that sets the request id, the trace id, the operation
    // and the graph on every log line of the request. A task the request spawns inherits it.
    let span =
        medulla_observe::trace::request_span(req.id, req.trace_id.as_deref(), method, &req.graph);
    let _ = span
        .in_scope(|| request_guard.apply(medulla_observe::machine::request::RequestEvent::Admit));
    let resp = medulla_observe::registry::requests::scope(
        request_guard.handle(),
        tracing::Instrument::instrument(dispatch_inner(state, req, context), span.clone()),
    )
    .await;
    span.in_scope(|| {
        if resp.error.is_some() {
            let _ = request_guard.apply(medulla_observe::machine::request::RequestEvent::Refuse);
        } else if request_guard
            .apply(medulla_observe::machine::request::RequestEvent::Respond)
            .is_ok()
        {
            let _ = request_guard.apply(medulla_observe::machine::request::RequestEvent::Complete);
        }
    });

    if let Some(start) = start {
        let elapsed = start.elapsed();
        #[cfg(feature = "metrics")]
        medulla_kernel::metrics::process().record_request(op, elapsed.as_secs_f64());
        if let Some(slow) = slow {
            slow.log_if_slow(elapsed);
        }
    }
    method_errors::enforce(method, resp)
}

async fn dispatch_inner(
    state: &Arc<RwLock<ServerState>>,
    mut req: Request,
    context: Option<VerifiedRequestContext>,
) -> Response {
    // External requests verify the current signed context. Mutation nonces are
    // carried into the durable MutationBatch kernel, while read-only nonces are
    // checked by the transport replay ledger before dispatch. In-process broker
    // bridges provide a context only after their protocol-specific credential has
    // verified.
    let verified_context = match context {
        Some(context) => context,
        None => {
            let s = timed_read(state).await;
            match s.request_authority(&req) {
                Ok(context) => context,
                Err(error) => {
                    medulla_kernel::metrics::process().auth_failure();
                    return Response::err(
                        req.id,
                        medulla_server_auth::auth::authentication_refusal(&error),
                    );
                }
            }
        }
    };
    if timed_read(state).await.local_anonymous == verified_context.is_authenticated() {
        return Response::err(req.id, "AUTHENTICATION_REQUIRED: authority mode mismatch");
    }
    if verified_context.is_sandbox_tenant() {
        if let Err(refusal) = admit_sandbox_agent(state, &verified_context).await {
            return Response::err(req.id, refusal);
        }
    }
    // MEDULLA-IDENTITY-R744: an envelope that names an agent-run delegation runs as the
    // delegation's principal, bounded to its operations. Resolved once here, before the scope
    // gate and every later check, so none of them ever sees the host's wider authority.
    let verified_context =
        match delegated_actor::resolve_delegated_actor(state, &req, verified_context).await {
            Ok(context) => context,
            Err(refusal) => return refusal,
        };
    req.agent_id = Some(verified_context.agent_id().to_string());

    #[cfg(feature = "raft")]
    let state_machine_authorized = medulla_kernel::kernel::dispatch_support::is_replicated_apply();
    #[cfg(not(feature = "raft"))]
    let state_machine_authorized = false;

    // ── Scope + admin enforcement (CONCEPT:MEDULLA-KG.compute.feature, MEDULLA-P0-6) ────────────────
    // A verified context must carry the capability ledger's action scope.
    // Additionally gates EVERY method whose policy declares a system-wide
    // admin `authz_action` (RegisterIdentity/RbacAdmin/ApplyMultisigMutation, the
    // M3 reshard/rebalance/catalog family, Backup/Restore) behind
    // `IsolationLayer::has_admin_capability` -- driven off the ledger's
    // `authz_action` string (`access::is_admin_authz_action`), NEVER a second
    // hardcoded method-name list here. Checked ONCE, before the method match, so
    // every current AND future admin-tier method is covered without a dispatch.rs
    // edit. Runs only when the `server` feature (which pulls in `medulla-capabilities`)
    // is active -- always true for this binary.
    let (req, authority) =
        match dispatch_preamble_checks(state, req, &verified_context, state_machine_authorized)
            .await
        {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    // The caller is authorized for the method: the request machine moves on.
    medulla_observe::registry::requests::advance(
        medulla_observe::machine::request::RequestEvent::Authorize,
    );
    dispatch_admitted_request(state, req, &verified_context, authority).await
}

async fn dispatch_admitted_request(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    verified_context: &VerifiedRequestContext,
    authority: DispatchAuthority,
) -> Response {
    // The handler side begins: everything from here to the response is `Handling`.
    medulla_observe::registry::requests::advance(
        medulla_observe::machine::request::RequestEvent::Handle,
    );
    let session_control = match begin_session_control_saga(
        state,
        req.id,
        verified_context,
        &req.method,
        verified_context.attempt_nonce(),
    )
    .await
    {
        Ok(control) => control,
        Err(error) => return Response::err(req.id, error),
    };
    if let Some(control) = session_control.as_ref() {
        #[cfg(feature = "redb")]
        if let Some(response) = replayed_response(req.id, control, &req.method) {
            return response;
        }
        #[cfg(not(feature = "redb"))]
        let _ = control;
    }

    #[cfg(feature = "redb")]
    let saga_authority = match CarrierAuthority::from_verified(verified_context) {
        Ok(authority) => authority,
        Err(error) => return Response::err(req.id, error),
    };
    let operation = async {
        let req_id = req.id;
        let method_for_finalize = req.method.clone();
        // `dispatch_request_method` returns a boxed `Send` future, so the whole
        // router tree stays out of this future's concrete type.
        let response = dispatch_request_method(state, req, verified_context, authority).await;
        finalize_dispatch_response(req_id, response, &method_for_finalize, session_control)
    };
    #[cfg(feature = "redb")]
    return medulla_server_ops::api::admin::saga::scope_admin_saga_authority(
        saga_authority,
        operation,
    )
    .await;
    #[cfg(not(feature = "redb"))]
    operation.await
}
