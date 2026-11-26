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
    let method = super::memory_change::dispatch_consolidate_method(ctx, method).await?;
    let method = super::memory_change::dispatch_sync_memory_method(ctx, method).await?;
    let method = dispatch_source_sync_method(ctx, method).await?;
    let method = dispatch_process_model_deploy_method(ctx, method).await?;
    let method = dispatch_decision_methods(ctx, method).await?;
    let method = dispatch_write_back_method(ctx, method).await?;
    #[cfg(feature = "security")]
    let method = dispatch_data_product_methods(ctx, method).await?;
    dispatch_catalog_admin_methods(ctx, method).await
}

/// Commit the batch a decision handler planned (MEDULLA-DECISION-ENGINE-R701.3)
/// as one `BatchUpdate` through the graph gateway under the caller's verified
/// context, exactly as the incident handlers commit. A refused commit answers
/// the refusal and never the reply; an empty plan commits nothing and answers
/// the reply.
async fn commit_planned_write(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    plan: medulla_h_decide::decide::planned_write::PlannedWrite,
) -> Response {
    if !plan.operations.is_empty() {
        let operations_msgpack = match rmp_serde::to_vec_named(&plan.operations) {
            Ok(bytes) => bytes,
            Err(_) => return Response::err(req_id, "planned write encoding failed"),
        };
        let response = dispatch_graph_op(
            state,
            &plan.graph,
            req_id,
            Some(verified.agent_id()),
            verified,
            Method::BatchUpdate { operations_msgpack },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Response::err(req_id, refusal);
        }
    }
    Response::ok(req_id, plan.reply)
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

/// `Method::ProcessModelDeploy` (MEDULLA-CQ-ORCH-R703.3): the graph side exports the model and
/// answers one outbound draft bound to the document digest; this router step queues it as a
/// write-back proposal through the same `queue_outbound` as a sync (MEDULLA-SRC-AUTH-R012.5).
/// Deploy never calls the source: the connector deploys the proposal once it is released.
async fn dispatch_process_model_deploy_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ProcessModelDeploy { connection_iri, .. } = &method else {
        return ControlFlow::Continue(method);
    };
    let connection = connection_iri.clone();
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
            medulla_types::process_exchange::PROCESS_MODEL_CLASS_IRI,
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
                queue_id: queue_id.clone(),
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
    report.queue_id = Some(queue_id);
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
        Method::AgentExecutionAnswer { request } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_agent_execution_answer(
                    state,
                    req.id,
                    (&req.graph, verified_context),
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
        Method::FailureIngest { op } => {
            dispatch_boxed(async {
                match medulla_h_decide::decide::failure_ingest::handle_failure_ingest(
                    state,
                    verified_context,
                    *op,
                )
                .await
                {
                    Ok(plan) => commit_planned_write(state, req.id, verified_context, plan).await,
                    Err(refusal) => Response::err(req.id, refusal),
                }
            })
            .await
        }
        Method::Usage { op } => {
            dispatch_boxed(async {
                medulla_h_decide::usage::handle_usage(state, req.id, verified_context, *op).await
            })
            .await
        }
        Method::FeatureLedger { op } => {
            dispatch_boxed(async {
                medulla_h_decide::decide::handle_feature_ledger(
                    state,
                    req.id,
                    verified_context,
                    *op,
                )
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
            // MEDULLA-MEDIA-VOCAB-R001.102.2: an import that leaves a pack declaring `Bookmark`
            // authors the tenant's `bookmark` semantic binding.
            #[cfg(all(feature = "ann-redb", feature = "query"))]
            let imported = match op.as_ref() {
                medulla_catalog_types::connector_pack::ConnectorPackOp::Import { request } => {
                    Some(request.index.connector.as_str().to_string())
                }
                _ => None,
            };
            let response = dispatch_boxed(async {
                medulla_h_admin::admin::handle_connector_pack(state, req.id, verified_context, *op)
                    .await
            })
            .await;
            #[cfg(all(feature = "ann-redb", feature = "query"))]
            if let Some(connector) = imported.filter(|_| response.refusal_text().is_none()) {
                if let Err(refusal) = super::bookmark_binding::author_on_pack_activation(
                    state,
                    &req.graph,
                    verified_context,
                    &connector,
                )
                .await
                {
                    return ControlFlow::Break(Response::err(req.id, refusal));
                }
            }
            response
        }
        // MEDULLA-TYPED-PACKS-R097.700: `Preview` evaluates the manifest and writes
        // nothing; `Run` continues to the mutation gateway (`Route::DomainPack`).
        Method::DomainPack { op } if !op.is_mutation() => {
            dispatch_boxed(async {
                medulla_h_graph::graph_ops::domain_pack::handle_domain_pack(
                    state,
                    req.id,
                    &req.graph,
                    verified_context,
                    *op,
                )
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
        // MEDULLA-TYPED-PACKS-R108.3: a registration commits through the graph gateway, then
        // the registered definition is cataloged by the one emitter.
        Method::WorkflowRegistry { op } => {
            let registration = match op.as_ref() {
                medulla_work_types::orchestration::workflows::WorkflowRegistryOp::Register {
                    registration,
                } => Some(registration.clone()),
                medulla_work_types::orchestration::workflows::WorkflowRegistryOp::Read { .. } => {
                    None
                }
            };
            let response = dispatch_boxed(dispatch_graph_op(
                state,
                &req.graph,
                req.id,
                req.agent_id.as_deref(),
                verified_context,
                Method::WorkflowRegistry { op },
            ))
            .await;
            #[cfg(feature = "rdf")]
            if let (Some(registration), None) = (&registration, response.refusal_text()) {
                let emitted = super::catalog_emit::commit_asset(
                    state,
                    req,
                    verified_context,
                    &req.graph,
                    medulla_catalog::emit::AssetRecord::WorkflowDefinition(registration),
                )
                .await;
                return ControlFlow::Break(super::catalog_emit::after_emit(
                    response, req.id, emitted,
                ));
            }
            let _ = registration;
            response
        }
        // MEDULLA-TYPED-PACKS-R108.14: a published stored-query version is cataloged by the one
        // emitter, as a descriptor and a link; the text stays on its own node.
        Method::StoredQueryPut { query } => {
            let published = query.clone();
            let response = dispatch_boxed(dispatch_graph_op(
                state,
                &req.graph,
                req.id,
                req.agent_id.as_deref(),
                verified_context,
                Method::StoredQueryPut { query },
            ))
            .await;
            #[cfg(feature = "rdf")]
            if response.refusal_text().is_none() {
                let emitted = super::catalog_emit::commit_asset(
                    state,
                    req,
                    verified_context,
                    &req.graph,
                    medulla_catalog::emit::AssetRecord::StoredQuery(&published),
                )
                .await;
                return ControlFlow::Break(super::catalog_emit::after_emit(
                    response, req.id, emitted,
                ));
            }
            let _ = published;
            response
        }
        // MEDULLA-TYPED-PACKS-R801.3: the glossary ops commit through the graph gateway.
        Method::Glossary { op } => {
            dispatch_boxed(async {
                medulla_server_graph::dispatch::glossary::dispatch_glossary(
                    state,
                    medulla_server_graph::dispatch::graph_client::GraphTarget {
                        graph: &req.graph,
                        req_id: req.id,
                        caller: req.agent_id.as_deref(),
                        verified: verified_context,
                    },
                    *op,
                )
                .await
            })
            .await
        }
        // MEDULLA-TYPED-PACKS-R802.3: the stewardship ops commit through the graph gateway.
        Method::Stewardship { op } => {
            dispatch_boxed(async {
                let target = medulla_server_graph::dispatch::graph_client::GraphTarget {
                    graph: &req.graph,
                    req_id: req.id,
                    caller: req.agent_id.as_deref(),
                    verified: verified_context,
                };
                let applied = match op.as_ref() {
                    medulla_catalog_types::stewardship::StewardshipOp::Apply { request_id } => {
                        Some(request_id.clone())
                    }
                    _ => None,
                };
                let response = medulla_server_graph::dispatch::stewardship::dispatch_stewardship(
                    state, target, *op,
                )
                .await;
                // MEDULLA-TYPED-PACKS-R107.6: an applied change request that holds a pack
                // release activates it through `ContractHistory::approve`, the one path.
                match applied {
                    Some(request_id) if response.refusal_text().is_none() => {
                        activate_held_pack_release(state, req, verified_context, &request_id, response)
                            .await
                    }
                    _ => response,
                }
            })
            .await
        }
        // MEDULLA-TYPED-PACKS-R109.3: the dashboard ops commit through the graph gateway.
        Method::DashboardAsset { op } => {
            dispatch_boxed(async {
                medulla_server_graph::dispatch::dashboard::dispatch_dashboard(
                    state,
                    medulla_server_graph::dispatch::graph_client::GraphTarget {
                        graph: &req.graph,
                        req_id: req.id,
                        caller: req.agent_id.as_deref(),
                        verified: verified_context,
                    },
                    *op,
                )
                .await
            })
            .await
        }
        Method::PublishSkillPackage { request } => {
            dispatch_boxed(async {
                medulla_h_admin::admin::handle_publish_skill_package(
                    state,
                    req.id,
                    verified_context,
                    *request,
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

/// Activate the pack release held behind the change request just applied, when it holds one.
/// A failed activation answers its refusal: the request stays applied and a repeat of the
/// apply finds nothing held.
async fn activate_held_pack_release(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    request_id: &str,
    applied: Response,
) -> Response {
    let target = medulla_server_graph::dispatch::graph_client::GraphTarget {
        graph: &req.graph,
        req_id: req.id,
        caller: req.agent_id.as_deref(),
        verified,
    };
    let (subject, digest) = match medulla_server_graph::dispatch::stewardship::request_subject(
        state, &target, request_id,
    )
    .await
    {
        Ok(subject) => subject,
        Err(refusal) => return Response::err(req.id, refusal),
    };
    match medulla_h_admin::admin::activate_held_release(state, req.id, verified, &subject, &digest)
        .await
    {
        // MEDULLA-MEDIA-VOCAB-R001.102.2: a released pack that declares `Bookmark` authors the
        // tenant's `bookmark` semantic binding.
        #[cfg(all(feature = "ann-redb", feature = "query"))]
        Ok(Some(_)) => {
            let connector = subject.strip_prefix("pack:").unwrap_or(&subject);
            match super::bookmark_binding::author_on_pack_activation(
                state, &req.graph, verified, connector,
            )
            .await
            {
                Ok(()) => applied,
                Err(refusal) => Response::err(req.id, refusal),
            }
        }
        Ok(_) => applied,
        Err(refusal) if refusal.starts_with("PACK_RELEASE_NOT_HELD") => applied,
        Err(refusal) => Response::err(req.id, refusal),
    }
}
