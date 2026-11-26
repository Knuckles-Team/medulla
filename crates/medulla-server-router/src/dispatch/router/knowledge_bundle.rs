//! `Method::BundleExport` and `Method::BundleImport` (MEDULLA-DURABLE-KERNEL-R877.4).
//!
//! The handlers live in `medulla_h_admin::knowledge_bundle`. The export answers from there. The
//! import is verified there and committed here: each target graph is created through the graph
//! lifecycle path, its triples commit through the `AddTriples` gateway, and its provenance rows
//! and embeddings commit as one `BatchUpdate`. The import is one `importing` control-graph record
//! and each graph it creates carries the import id on its durable registry row, written with the
//! creation: `check_graph_access` hides the graph until the import clears that marker, and the
//! import's own steps run under the import id (`importing::within_import`). The record is set
//! `ready` last. A refused step deletes every graph this import created. A crash leaves the record
//! `importing` and the marked graphs hidden, in this process and after a restart, and the leader
//! tick (`medulla_h_jobs`, bundle import recovery) deletes the graphs the record names.

use super::*;
#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
use medulla_server_auth::access::importing;
#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
use medulla_types::knowledge_bundle::{ImportRecord, ImportState};

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
        let targets: Vec<String> = plan
            .graphs
            .iter()
            .filter(|graph| !graph.skip)
            .map(|graph| graph.target.clone())
            .collect();
        // MEDULLA-DURABLE-KERNEL-R877.3: the import is one control-graph record. Every graph it
        // creates is hidden from every read while the record says `importing`; one write of the
        // record flips them all to `ready`. A crash leaves the record `importing`, and the
        // leader tick deletes the graphs it names.
        let record = ImportRecord::begin(
            verified.tenant(),
            &bundle_ref,
            &target_graph_prefix,
            verified.idempotency_key(),
            targets.clone(),
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        );
        let control = medulla_server_api::knob_config::default_ingest_graph();
        let target = medulla_server_graph::dispatch::graph_client::GraphTarget {
            graph: control,
            req_id: req.id,
            caller: req.agent_id.as_deref(),
            verified,
        };
        let agent = verified.agent_id();
        let write_record = |record: ImportRecord| {
            let operation = record.node_operation(agent);
            let target = &target;
            async move {
                medulla_server_graph::dispatch::graph_client::write_batch(state, target, &[operation])
                    .await
            }
        };
        // The importing record is durable before any graph exists.
        if let Err(refusal) = write_record(record.clone()).await {
            return Response::err(req.id, refusal);
        }
        importing::mark_live(&record.import_id);
        let outcome = importing::within_import(record.import_id.clone(), async {
            let mut created: Vec<String> = Vec::new();
            let committed =
                commit_plan(state, req, verified, &plan, &record.import_id, &mut created).await;
            // Every graph's marker clears first and the record is set `ready` last, so a crash
            // between the two finds the record `importing` and the recovery pass deletes the
            // whole import, never a part of it.
            let committed = match committed {
                Ok(()) => {
                    let mut cleared = Ok(());
                    for graph in &targets {
                        if let Err(error) = mark_graph_ready(state, graph).await {
                            cleared = Err(error);
                            break;
                        }
                    }
                    match cleared {
                        Ok(()) => write_record(record.with_state(ImportState::Ready)).await,
                        Err(refusal) => Err(refusal),
                    }
                }
                Err(refusal) => Err(refusal),
            };
            if committed.is_err() {
                // Every graph this import created goes again, newest first.
                for graph in created.iter().rev() {
                    if let Ok(request) = rollback_request(req, verified, graph) {
                        let _ = delete_graph(state, request, state_machine_authorized, graph).await;
                    }
                }
                let _ = write_record(record.with_state(ImportState::Aborted)).await;
            }
            committed
        })
        .await;
        importing::mark_done(&record.import_id);
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
            Err(refusal) => Response::err(req.id, refusal),
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
    import_id: &str,
    created: &mut Vec<String>,
) -> Result<(), String> {
    for graph in plan.graphs.iter().filter(|graph| !graph.skip) {
        let request = graph_request(req, verified, "create", &graph.target)?.importing(import_id);
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
