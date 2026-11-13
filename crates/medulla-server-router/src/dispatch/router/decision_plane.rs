use super::*;

/// The decision plane: the Decide layer, the solver, the connector pack and
/// the mutation-outbox admin surface.
///
/// One link per group, chained with `?` on `ControlFlow` exactly like the
/// control plane above it: `Break(response)` short-circuits, `Continue(method)`
/// hands the method to the next group.
pub async fn dispatch_decision_plane_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let method = dispatch_source_sync_method(ctx, method).await?;
    let method = dispatch_decision_methods(ctx, method).await?;
    let method = dispatch_write_back_method(ctx, method).await?;
    dispatch_catalog_admin_methods(ctx, method).await
}

async fn dispatch_write_back_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::WriteBack { op } = method else {
        return ControlFlow::Continue(method);
    };
    #[cfg(feature = "redb")]
    {
        ControlFlow::Break(
            dispatch_boxed(async {
                medulla_h_graph::write_back::handle_write_back(
                    ctx.state,
                    ctx.req.id,
                    ctx.verified_context,
                    *op,
                )
                .await
            })
            .await,
        )
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (ctx.state, ctx.verified_context, op);
        ControlFlow::Break(Response::err(
            ctx.req.id,
            "write-back requires the `redb` feature",
        ))
    }
}

/// `Method::SourceSync` (MEDULLA-SRC-AUTH-R012.5, R012.6): the graph side commits
/// the inbound changes and the sync state in one batch and answers the outbound
/// drafts; this router step queues them as write-back proposals. Sync never
/// writes to a source itself.
async fn dispatch_source_sync_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::SourceSync {
        connection_iri,
        class_iri,
        ..
    } = &method
    else {
        return ControlFlow::Continue(method);
    };
    let (connection, class) = (connection_iri.clone(), class_iri.clone());
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    let response = dispatch_boxed(dispatch_graph_op(
        state,
        &req.graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        method,
    ))
    .await;
    ControlFlow::Break(
        dispatch_boxed(queue_outbound(
            state,
            req.id,
            &req.graph,
            verified_context,
            &connection,
            &class,
            response,
        ))
        .await,
    )
}

/// Queue every outbound draft of a sync answer as one write-back proposal
/// (MEDULLA-SRC-AUTH-R012.5). The queue id is the SHA-256 of the connection, the
/// class and the sorted payload digests, so a re-sync of an unchanged graph
/// proposes the same queue: `WRITEBACK_QUEUE_EXISTS` counts as already queued,
/// which makes commit-then-queue self-healing (a proposed record keeps its
/// earlier base, so the next sync proposes it again under the same id). Every
/// sync outbound change is high tier: it is approved before release.
async fn queue_outbound(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    connection: &str,
    class: &str,
    response: Response,
) -> Response {
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    use medulla_decision_types::decision::statistical::writeback::{
        ProposalInput, QueuedProposalInput, RiskTier, WritebackOp,
    };
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::graph::SourceSync;
    use medulla_source_types::source_sync::SyncReport;

    if response.error.is_some() {
        return response;
    }
    let Some(ResultPayload::Json(value)) = &response.result else {
        return response;
    };
    let mut report: SyncReport = match serde_json::from_value(value.clone()) {
        Ok(report) => report,
        Err(error) => {
            return Response::err(
                req_id,
                format!("INVALID_ARGUMENT: the sync report is undecodable: {error}"),
            )
        }
    };
    if report.outbound.is_empty() {
        return response;
    }
    let mut digests: Vec<&str> = report
        .outbound
        .iter()
        .map(|draft| draft.payload_digest.as_str())
        .collect();
    digests.sort_unstable();
    let mut framed = Vec::new();
    for part in [connection, class].into_iter().chain(digests) {
        framed.extend_from_slice(part.as_bytes());
        framed.push(0);
    }
    let queue_id = medulla_types::contract::Digest256::sha256(&framed).to_hex();
    let prefix = &queue_id[..16];
    let as_of_ms = i64::try_from(medulla_kernel::kernel::dispatch_support::authoritative_now_ms())
        .unwrap_or(i64::MAX);
    let proposals: Vec<QueuedProposalInput> = report
        .outbound
        .iter()
        .map(|draft| QueuedProposalInput {
            proposal: ProposalInput {
                id: format!("sync:{prefix}:{}", draft.record_key),
                external_id: draft.record_key.clone(),
                system_of_record: graph.to_string(),
                risk_tier: RiskTier::High,
                as_of_ms,
            },
            source_values: std::collections::BTreeMap::new(),
        })
        .collect();
    let proposal_ids: Vec<String> = proposals
        .iter()
        .map(|queued| queued.proposal.id.clone())
        .collect();
    let queued = medulla_h_decide::decide::handle_decision_log(
        state,
        req_id,
        verified,
        DecisionLogOp::Writeback {
            tenant_id: verified.tenant().to_string(),
            computation: WritebackOp::Queue {
                queue_id,
                proposals,
            },
        },
    )
    .await;
    if let Some(refusal) = queued.refusal_text() {
        if !refusal.starts_with("WRITEBACK_QUEUE_EXISTS") {
            return Response::err(req_id, refusal);
        }
    }
    report.queued = proposal_ids;
    match ResultPayload::of::<SourceSync>(report) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The four Decide methods and the solver. None carries a `#[cfg]`: the wire
/// contract is unconditional, and each handler owns whatever feature gate its
/// own implementation needs.
async fn dispatch_decision_methods(
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
        Method::AgentAssemble { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_agent_assemble(
                    state,
                    req.id,
                    (&req.graph, verified_context),
                    *request,
                )
                .await
            })
            .await
        }
        Method::CapabilityCoverage { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_capability_coverage(
                    state,
                    req.id,
                    verified_context,
                    *request,
                )
                .await
            })
            .await
        }
        Method::RevisionProposalList { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_revision_proposal_list(
                    state,
                    req.id,
                    verified_context,
                    *request,
                )
                .await
            })
            .await
        }
        Method::GuardrailEntailment { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_guardrail_entailment(
                    state,
                    req.id,
                    verified_context,
                    *request,
                )
                .await
            })
            .await
        }
        Method::DecisionCommit { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_decision_commit(
                    state,
                    req.id,
                    verified_context,
                    *request,
                )
                .await
            })
            .await
        }
        Method::Decide { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_decide(state, req.id, verified_context, *request)
                    .await
            })
            .await
        }
        Method::DecisionFit { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_decision_fit(state, req.id, verified_context, *op)
                    .await
            })
            .await
        }
        Method::DecisionEval { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_decision_eval(state, req.id, verified_context, *op)
                    .await
            })
            .await
        }
        Method::DecisionLog { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_decision_log(state, req.id, verified_context, *op)
                    .await
            })
            .await
        }
        Method::BeliefRevise { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_belief_revise(
                    state,
                    req.id,
                    verified_context,
                    *request,
                )
                .await
            })
            .await
        }
        Method::Claims { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_claims(state, req.id, verified_context, *op).await
            })
            .await
        }
        Method::Loops { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_loops(state, req.id, verified_context, *op).await
            })
            .await
        }
        Method::Solve { request } => {
            dispatch_boxed(async { medulla_h_decide::solve::handle_solve(req.id, *request).await })
                .await
        }
        other => return ControlFlow::Continue(other),
    })
}

/// Catalog administration: connector packs, governed write-back records and
/// the mutation outbox.
async fn dispatch_catalog_admin_methods(
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
        Method::ConnectorPack { op } => {
            dispatch_boxed(async {
                medulla_h_admin::admin::handle_connector_pack(state, req.id, verified_context, *op)
                    .await
            })
            .await
        }
        // MEDULLA-DECISION-ENGINE-R166.3: `Read` is a graph read served here and
        // holds no write; `Register` continues to the mutation gateway.
        Method::WorkflowRegistry { op } if !op.is_mutation() => {
            let medulla_work_types::orchestration::workflows::WorkflowRegistryOp::Read { identity } =
                *op
            else {
                unreachable!("only Register mutates");
            };
            dispatch_boxed(async {
                medulla_h_admin::admin::handle_workflow_registry_read(
                    state,
                    req.id,
                    &req.graph,
                    verified_context,
                    identity,
                )
                .await
            })
            .await
        }
        Method::MutationOutbox { op } => {
            dispatch_boxed(async {
                medulla_h_jobs::mutation_outbox::handle_mutation_outbox(
                    state,
                    req.id,
                    verified_context,
                    *op,
                )
                .await
            })
            .await
        }
        other => return ControlFlow::Continue(other),
    })
}
