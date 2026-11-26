//! `Method::BundleExport` and `Method::BundleImport` (MEDULLA-DURABLE-KERNEL-R877.4).
//!
//! The handlers live in `medulla_h_admin::knowledge_bundle`. The export answers from there. The
//! import is verified there and committed here: each target graph is created through the graph
//! lifecycle path, its triples commit through the `AddTriples` gateway, and its provenance rows
//! and embeddings commit as one `BatchUpdate`. A refused step deletes every graph this import
//! created, so a refusal leaves the target unchanged.

use super::*;

/// Serve `Method::BundleExport`.
pub(super) async fn serve_bundle_export(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    graphs: Vec<String>,
    include_embeddings: bool,
    include_documents: bool,
) -> Response {
    #[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
    {
        medulla_h_admin::knowledge_bundle::handle_bundle_export(
            state,
            req_id,
            verified,
            graphs,
            include_embeddings,
            include_documents,
        )
        .await
    }
    #[cfg(not(all(feature = "rdf", feature = "blob", feature = "redb")))]
    {
        let _ = (state, verified, graphs, include_embeddings, include_documents);
        Response::err(req_id, "BundleExport requires the `rdf`, `blob` and `redb` features")
    }
}

/// Serve `Method::BundleImport`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn serve_bundle_import(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    state_machine_authorized: bool,
    bundle_ref: String,
    target_graph_prefix: String,
    on_conflict: medulla_types::knowledge_bundle::BundleOnConflict,
) -> Response {
    #[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
    {
        let plan = match medulla_h_admin::knowledge_bundle::plan_import(
            state,
            verified,
            &bundle_ref,
            &target_graph_prefix,
            on_conflict,
        )
        .await
        {
            Ok(plan) => plan,
            Err(refusal) => return Response::err(req.id, refusal),
        };
        let mut created: Vec<String> = Vec::new();
        let outcome = commit_plan(state, req, verified, &plan, &mut created).await;
        match outcome {
            Ok(()) => {
                match ResultPayload::of::<
                    medulla_protocol_types::result_contract::cluster::BundleImport,
                >(plan.receipt())
                {
                    Ok(payload) => Response::ok(req.id, payload),
                    Err(error) => Response::err(req.id, error),
                }
            }
            Err(refusal) => {
                // Every graph this import created goes again, newest first.
                for graph in created.iter().rev() {
                    if let Ok(request) = rollback_request(req, verified, graph) {
                        let _ = delete_graph(state, request, state_machine_authorized, graph).await;
                    }
                }
                Response::err(req.id, refusal)
            }
        }
    }
    #[cfg(not(all(feature = "rdf", feature = "blob", feature = "redb")))]
    {
        let _ = (state, verified, state_machine_authorized, bundle_ref, target_graph_prefix, on_conflict);
        Response::err(req.id, "BundleImport requires the `rdf`, `blob` and `redb` features")
    }
}

#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
fn graph_request(
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    purpose: &str,
    graph: &str,
) -> Result<GraphLifecycleRequest, String> {
    // The lifecycle step is its own durable operation: it replays under its own key and never
    // consumes the caller's attempt nonce.
    let key = format!("bundle-{purpose}:{graph}:{}", verified.idempotency_key());
    GraphLifecycleRequest::verified(req.id, req.agent_id.clone(), verified, key)
        .map(GraphLifecycleRequest::without_attempt_nonce)
}

#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
fn rollback_request(
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    graph: &str,
) -> Result<GraphLifecycleRequest, String> {
    graph_request(req, verified, "rollback", graph)
}

/// The refusal text of a response that refused, if it did.
#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
fn refused(response: &Response) -> Option<String> {
    response.refusal_text().map(str::to_string)
}

#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
async fn commit_plan(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    plan: &medulla_h_admin::knowledge_bundle::ImportPlan,
    created: &mut Vec<String>,
) -> Result<(), String> {
    for graph in plan.graphs.iter().filter(|graph| !graph.skip) {
        let request = graph_request(req, verified, "create", &graph.target)?;
        let response = dispatch_boxed(create_graph(
            state,
            request,
            graph.target.clone(),
            medulla_protocol_types::protocol::GraphType::Agent,
        ))
        .await;
        if let Some(refusal) = refused(&response) {
            return Err(refusal);
        }
        created.push(graph.target.clone());
        if let Some(space) = &graph.space {
            let method = Method::DeclareEmbeddingSpace {
                space: serde_json::from_value(space.clone())
                    .map_err(|error| format!("PARAMETER_INVALID: the embedding space is malformed: {error}"))?,
            };
            let response = dispatch_boxed(dispatch_graph_op(
                state,
                &graph.target,
                req.id,
                req.agent_id.as_deref(),
                verified,
                method,
            ))
            .await;
            if let Some(refusal) = refused(&response) {
                return Err(refusal);
            }
        }
        if !graph.ntriples.is_empty() {
            let method = Method::AddTriples {
                turtle: String::new(),
                ntriples: graph.ntriples.clone(),
            };
            let response = dispatch_boxed(dispatch_graph_op(
                state,
                &graph.target,
                req.id,
                req.agent_id.as_deref(),
                verified,
                method,
            ))
            .await;
            if let Some(refusal) = refused(&response) {
                return Err(refusal);
            }
        }
        if !graph.operations.is_empty() {
            let operations_msgpack = rmp_serde::to_vec_named(&graph.operations)
                .map_err(|error| format!("PARAMETER_INVALID: the import batch is not encodable: {error}"))?;
            let response = dispatch_boxed(dispatch_graph_op(
                state,
                &graph.target,
                req.id,
                req.agent_id.as_deref(),
                verified,
                Method::BatchUpdate { operations_msgpack },
            ))
            .await;
            if let Some(refusal) = refused(&response) {
                return Err(refusal);
            }
        }
    }
    Ok(())
}
