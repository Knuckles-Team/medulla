//! Private arm helpers for data-plane dispatch.

use super::*;

pub async fn dispatch_transaction_methods_arm_0(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ (Method::BeginTxn { .. }
        | Method::TxnAddNode { .. }
        | Method::TxnRemoveNode { .. }
        | Method::TxnAddEdge { .. }
        | Method::TxnRemoveEdge { .. }
        | Method::TxnCas { .. }
        | Method::TxnAddEmbedding { .. }
        | Method::TxnBlobRef { .. }
        | Method::Commit { .. }
        | Method::Rollback { .. }) => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_graph = req.graph.clone();
                {
                    // BeginTxn defaults its target to the request envelope's graph.
                    let method = match method {
                        Method::BeginTxn {
                            graph: None,
                            isolation,
                        } => Method::BeginTxn {
                            graph: Some(req_graph.clone()),
                            isolation,
                        },
                        m => m,
                    };
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        // Unreachable: every variant matched above is a txn method.
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "tsdb")]
pub async fn dispatch_transaction_methods_arm_1(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ Method::TxnAddMeasurement { .. } => {
            dispatch_boxed(async {
                let req_id = req.id;
                {
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "owl")]
pub async fn dispatch_transaction_methods_arm_2(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ Method::TxnAxiom { .. } => {
            dispatch_boxed(async {
                let req_id = req.id;
                {
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "sparql")]
pub async fn dispatch_transaction_methods_arm_3(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ Method::TxnConstruct { .. } => {
            dispatch_boxed(async {
                let req_id = req.id;
                {
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "query")]
pub async fn dispatch_transaction_methods_arm_4(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ Method::TxnPlanWriteback { .. } => {
            dispatch_boxed(async {
                let req_id = req.id;
                {
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

#[cfg(feature = "epistemic")]
pub async fn dispatch_transaction_methods_arm_5(ctx: DispatchCtx<'_>, method: Method) -> Response {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        identity_bootstrap,
    } = ctx;
    match method {
        method @ Method::TxnMaterializeBelief { .. } => {
            dispatch_boxed(async {
                let req_id = req.id;
                {
                    match medulla_h_txn::txn::try_handle(
                        state,
                        req_id,
                        verified_context.agent_id(),
                        verified_context,
                        method,
                    )
                    .await
                    {
                        Ok(resp) => resp,
                        Err(_) => Response::err(req_id, "txn dispatch routing error"),
                    }
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

/// The first envelope field that differs from the verified request carrying
/// it: graph, tenant, request number, then actor. A native (non-graph)
/// mutation scope reports no graph name at all; comparing `Option<&str>`
/// against `Some(graph)` fails closed on `None` instead of coercing it into an
/// empty-string match. The idempotency key and policy need no comparison: the
/// request boundary mints the envelope's authority from this same verified
/// context, and the real policy revision is inside the stable replay identity,
/// where a change conflicts rather than merely mismatching here.
fn envelope_context_mismatch(
    envelope: &medulla_protocol_types::change_envelope::ChangeEnvelope,
    graph: &str,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
) -> Option<&'static str> {
    let identity = &envelope.mutation.identity;
    if identity
        .scope()
        .graph_name()
        .map(medulla_mutation_types::mutation_scope::LogicalName::as_str)
        != Some(graph)
    {
        return Some("graph");
    }
    if identity.tenant().as_str() != verified.claims().tenant {
        return Some("tenant");
    }
    if medulla_protocol_types::mutation_batch::batch_request_number(&envelope.mutation)
        != Some(req_id)
    {
        return Some("request number");
    }
    if medulla_server_api::batch_compile::batch_actor(&envelope.mutation)
        != Some(verified.principal_persistence_id().as_str())
    {
        return Some("actor");
    }
    None
}

pub async fn dispatch_change_envelope_methods_arm_0(
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
        Method::ApplyChangeEnvelope { envelope } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                let req_graph = req.graph.clone();
                {
                    if let Some(field) =
                        envelope_context_mismatch(&envelope, &req_graph, req_id, verified_context)
                    {
                        return Response::err(
                            req_id,
                            format!(
                                "CHANGE_ENVELOPE_CONTEXT_MISMATCH: the envelope's {field} does not \
                                 match the verified request authority"
                            ),
                        );
                    }
                    dispatch_graph_op(
                        state,
                        &req_graph,
                        req_id,
                        req_agent_id.as_deref(),
                        verified_context,
                        Method::ApplyChangeEnvelope { envelope },
                    )
                    .await
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

pub async fn dispatch_change_envelope_methods_arm_1(
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
        Method::ApplyChangeEnvelopes { envelopes } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                {
                    dispatch_change_envelopes(
                        state,
                        req_id,
                        req_agent_id.as_deref(),
                        verified_context,
                        envelopes,
                    )
                    .await
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

pub async fn dispatch_change_envelope_methods_arm_2(
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
        Method::GetChangeEnvelope {
            envelope_id,
            tenant,
        } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                let req_graph = req.graph.clone();
                {
                    if tenant != verified_context.claims().tenant {
                        return Response::err(
                            req_id,
                            "ChangeEnvelope reads require the verified tenant context",
                        );
                    }
                    dispatch_graph_op(
                        state,
                        &req_graph,
                        req_id,
                        req_agent_id.as_deref(),
                        verified_context,
                        Method::GetChangeEnvelope {
                            envelope_id,
                            tenant,
                        },
                    )
                    .await
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

pub async fn dispatch_change_envelope_methods_arm_3(
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
        Method::GetContentVersion { object_id, tenant } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                let req_graph = req.graph.clone();
                {
                    if tenant != verified_context.claims().tenant {
                        return Response::err(
                            req_id,
                            "content-version reads require the verified tenant context",
                        );
                    }
                    dispatch_graph_op(
                        state,
                        &req_graph,
                        req_id,
                        req_agent_id.as_deref(),
                        verified_context,
                        Method::GetContentVersion { object_id, tenant },
                    )
                    .await
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}

pub async fn dispatch_change_envelope_methods_arm_4(
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
        Method::GetChangeCursor {
            source,
            partition,
            tenant,
        } => {
            dispatch_boxed(async {
                let req_id = req.id;
                let req_agent_id = req.agent_id.clone();
                let req_graph = req.graph.clone();
                {
                    if tenant != verified_context.claims().tenant {
                        return Response::err(
                            req_id,
                            "change-cursor reads require the verified tenant context",
                        );
                    }
                    dispatch_graph_op(
                        state,
                        &req_graph,
                        req_id,
                        req_agent_id.as_deref(),
                        verified_context,
                        Method::GetChangeCursor {
                            source,
                            partition,
                            tenant,
                        },
                    )
                    .await
                }
            })
            .await
        }
        _ => Response::err(req.id, "router dispatch helper routing mismatch"),
    }
}
