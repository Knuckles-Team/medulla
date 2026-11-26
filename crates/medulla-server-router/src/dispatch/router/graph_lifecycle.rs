use super::*;
use medulla_server_graph::dispatch::shared::timed_read;

/// Multi-tenant graph lifecycle: create, delete and list graphs.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_graph_lifecycle_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    #[allow(unused_variables)]
    let DispatchCtx {
        state,
        req,
        verified_context,
        state_machine_authorized,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        // ── Multi-tenant graph management ────────────────────────────
        Method::CreateGraph {
            graph_name,
            graph_type,
        } => match lifecycle_request(req, verified_context) {
            Ok(request) => {
                let created = dispatch_boxed(create_graph(
                    state,
                    request,
                    graph_name.clone(),
                    graph_type,
                ))
                .await;
                // MEDULLA-TYPED-PACKS-R108.3: a created graph is cataloged by the one emitter, in
                // the new graph, which the creator owns and which holds no shapes yet.
                #[cfg(feature = "rdf")]
                if created.refusal_text().is_none() {
                    let emitted = super::catalog_emit::commit_asset(
                        state,
                        req,
                        verified_context,
                        &graph_name,
                        medulla_catalog::emit::AssetRecord::Graph(
                            medulla_catalog::emit::GraphRegistration {
                                graph_id: &graph_name,
                                ontology_revision: None,
                                contract_digest: None,
                                sources: &[],
                            },
                        ),
                    )
                    .await;
                    return ControlFlow::Break(super::catalog_emit::after_emit(
                        created, req.id, emitted,
                    ));
                }
                created
            }
            Err(denied) => denied,
        },

        Method::DeleteGraph { graph_name } => match lifecycle_request(req, verified_context) {
            Ok(request) => {
                dispatch_boxed(delete_graph(
                    state,
                    request,
                    state_machine_authorized,
                    &graph_name,
                ))
                .await
            }
            Err(denied) => denied,
        },

        Method::ListGraphs => dispatch_boxed(list_graphs(state, verified_context, req.id)).await,
        // MEDULLA-DURABLE-KERNEL-R877.4: a knowledge bundle moves one scoped dataset between tenants.
        Method::BundleExport {
            graphs,
            include_embeddings,
            include_documents,
        } => {
            dispatch_boxed(super::knowledge_bundle::serve_bundle_export(
                state,
                req.id,
                verified_context,
                graphs,
                include_embeddings,
                include_documents,
            ))
            .await
        }
        Method::BundleImport {
            bundle_ref,
            target_graph_prefix,
            on_conflict,
        } => {
            dispatch_boxed(super::knowledge_bundle::serve_bundle_import(
                state,
                req,
                verified_context,
                state_machine_authorized,
                bundle_ref,
                target_graph_prefix,
                on_conflict,
            ))
            .await
        }
        // MEDULLA-BRANCH-MERGE-R001: one branch registry row.
        Method::CreateBranch {
            tenant,
            graph,
            name,
            base_engine_version,
        } => {
            create_branch(
                state,
                req.id,
                verified_context,
                (tenant, graph, name, base_engine_version),
            )
            .await
        }
        // MEDULLA-BRANCH-MERGE-R002: one mutation on a branch, then one commit.
        Method::BranchWrite {
            tenant,
            graph,
            branch,
            mutation,
        } => {
            dispatch_boxed(super::branch_write::handle_branch_write(
                state,
                req,
                verified_context,
                super::branch_write::BranchWriteFields {
                    tenant,
                    graph,
                    branch,
                    mutation: *mutation,
                },
            ))
            .await
        }
        // MEDULLA-BRANCH-MERGE-R003: merge base and outcome of two branch heads.
        Method::BranchMerge {
            tenant,
            graph,
            source,
            target,
            merge_base,
            policy,
        } => {
            super::branch_merge::handle_branch_merge(
                state,
                req,
                verified_context,
                (tenant, graph, source, target, merge_base, policy),
            )
            .await
        }
        // MEDULLA-BRANCH-MERGE-R006: the change feed between two chain versions.
        Method::BranchDiff {
            tenant,
            graph,
            branch,
            from_version,
            to_version,
            offset,
        } => {
            branch_diff(
                state,
                req.id,
                verified_context,
                (tenant, graph, branch, from_version, to_version, offset),
            )
            .await
        }
        // MEDULLA-BRANCH-MERGE-R006: a branch as of one engine version.
        Method::BranchReadAt {
            tenant,
            graph,
            branch,
            engine_version,
        } => {
            branch_read_at(
                state,
                req.id,
                verified_context,
                (tenant, graph, branch, engine_version),
            )
            .await
        }
        // MEDULLA-SCM-R002: one source changeset on a branch.
        // MEDULLA-SCM-R013.4: a symbol edit, projected and re-parsed before the accept path.
        Method::SubmitSymbolEdit { request } => {
            dispatch_boxed(super::submit_symbol_edit::handle_submit_symbol_edit(
                state,
                req,
                verified_context,
                super::submit_symbol_edit::SubmitSymbolEditFields { request },
            ))
            .await
        }
        Method::SubmitChangeset { request } => {
            dispatch_boxed(super::submit_changeset::handle_submit_changeset(
                state,
                req,
                verified_context,
                super::submit_changeset::SubmitChangesetFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R010: the first-parent line of a git history on a branch.
        Method::ImportGitHistory { request } => {
            dispatch_boxed(super::import_git_history::handle_import_git_history(
                state,
                req,
                verified_context,
                super::import_git_history::ImportGitHistoryFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R014: the changeset chain of a branch as a git pack.
        Method::GitExportPack { request } => {
            dispatch_boxed(super::git_export_pack::handle_git_export_pack(
                state,
                req,
                verified_context,
                super::git_export_pack::GitExportPackFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R004: the tree of a repository at one changeset revision.
        Method::ReadTree { request } => {
            dispatch_boxed(super::read_tree::handle_read_tree(
                state,
                req,
                verified_context,
                super::read_tree::ReadTreeFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R005.1: the changesets that touched one path, newest first.
        Method::ReadPathHistory { request } => {
            dispatch_boxed(super::read_path_history::handle_read_path_history(
                state,
                req,
                verified_context,
                super::read_path_history::ReadPathHistoryFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R005.2: the origin changeset of every line of one path.
        Method::ReadBlame { request } => {
            dispatch_boxed(super::read_blame::handle_read_blame(
                state,
                req,
                verified_context,
                super::read_blame::ReadBlameFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R009: the next changeset a bisect needs a verdict for.
        Method::BisectStep { request } => {
            dispatch_boxed(super::source_bisect::handle_bisect_step(
                state,
                req,
                verified_context,
                super::source_bisect::BisectStepFields { request },
            ))
            .await
        }
        // MEDULLA-SCM-R007: one principal's path grant on a repository.
        Method::SetPathGrant { request } => {
            dispatch_boxed(super::set_path_grant::handle_set_path_grant(
                state,
                req,
                verified_context,
                super::set_path_grant::SetPathGrantFields { request },
            ))
            .await
        }
        other => return ControlFlow::Continue(other),
    })
}

/// `CreateBranch` (MEDULLA-BRANCH-MERGE-R001), served only by a `redb` build.
async fn create_branch(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    fields: (String, String, String, Option<u64>),
) -> Response {
    #[cfg(feature = "redb")]
    {
        let (tenant, graph, name, base_engine_version) = fields;
        let request = medulla_h_graph::graph_ops::branch::CreateBranchRequest {
            tenant,
            graph,
            name,
            base_engine_version,
        };
        dispatch_boxed(medulla_h_graph::graph_ops::branch::handle_create_branch(
            state,
            req_id,
            verified_context,
            request,
        ))
        .await
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified_context, fields);
        Response::err(req_id, "CreateBranch requires the `redb` feature")
    }
}

/// `BranchDiff` (MEDULLA-BRANCH-MERGE-R006), served only by a `redb` build.
async fn branch_diff(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    fields: (String, String, String, Option<u64>, Option<u64>, u64),
) -> Response {
    #[cfg(feature = "redb")]
    {
        let (tenant, graph, branch, from_version, to_version, offset) = fields;
        let request = medulla_h_graph::graph_ops::branch::time_travel::BranchDiffRequest {
            tenant,
            graph,
            branch,
            from_version,
            to_version,
            offset,
        };
        dispatch_boxed(
            medulla_h_graph::graph_ops::branch::time_travel::handle_branch_diff(
                state,
                req_id,
                verified_context,
                request,
            ),
        )
        .await
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified_context, fields);
        Response::err(req_id, "BranchDiff requires the `redb` feature")
    }
}

/// `BranchReadAt` (MEDULLA-BRANCH-MERGE-R006), served only by a `redb` build.
async fn branch_read_at(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    fields: (String, String, String, u64),
) -> Response {
    #[cfg(feature = "redb")]
    {
        let (tenant, graph, branch, engine_version) = fields;
        let request = medulla_h_graph::graph_ops::branch::time_travel::BranchReadAtRequest {
            tenant,
            graph,
            branch,
            engine_version,
        };
        dispatch_boxed(
            medulla_h_graph::graph_ops::branch::time_travel::handle_branch_read_at(
                state,
                req_id,
                verified_context,
                request,
            ),
        )
        .await
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified_context, fields);
        Response::err(req_id, "BranchReadAt requires the `redb` feature")
    }
}

/// `ListGraphs`: every graph the verified caller may read, with its materialization
/// and index readiness.
async fn list_graphs(
    state: &Arc<RwLock<ServerState>>,
    verified_context: &VerifiedRequestContext,
    req_id: u64,
) -> Response {
    let s = timed_read(state).await;
    let read_authority = match GraphReadAuthority::from_verified(verified_context, &s.isolation) {
        Ok(authority) => authority,
        Err(denied) => return Response::err(req_id, denied),
    };
    let graphs: Vec<medulla_protocol_types::result_contract::cluster::GraphListing> = s
        .registry
        .list()
        .iter()
        .filter(|(name, _)| {
            // The catalog record, not the resident entry (#709): after a
            // restart every recovered graph is catalog-only until first
            // access, and `registry.get` is resident-only, so deciding from it
            // dropped every graph from the listing without an ACL decision.
            // The record keeps the graph's type and owner, so the decision is
            // the same one a resident graph gets.
            s.registry.catalog_record(name).is_some_and(|entry| {
                match check_graph_access(
                    &s.isolation,
                    read_authority.actor(),
                    name,
                    entry.graph_type,
                    entry.owner.as_deref(),
                    entry.importing.as_deref(),
                    AccessLevel::Read,
                ) {
                    Ok(()) => true,
                    Err(reason) => {
                        // A filtered graph is not an error to the caller, but the
                        // reason must not vanish (#709): an unprovisioned policy or
                        // an unregistered caller drops every graph at once. The
                        // reason names the refusal class and the graph only.
                        tracing::debug!(
                            target: "medulla_server_api::access::list_graphs",
                            graph = name.as_str(),
                            graph_type = ?entry.graph_type,
                            reason = %reason,
                            "graph filtered from ListGraphs"
                        );
                        false
                    }
                }
            })
        })
        .map(|(name, graph_type)| graph_listing(&s.registry, name, *graph_type))
        .collect();
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ListGraphs>(graphs),
    )
}

/// One readable graph's listing: its materialization readiness and index manifests.
fn graph_listing(
    registry: &medulla_core::registry::GraphRegistry,
    name: &str,
    graph_type: medulla_protocol_types::protocol::GraphType,
) -> medulla_protocol_types::result_contract::cluster::GraphListing {
    let readiness = registry.materialization_manifest(name);
    let index_manifests = registry
        .get(name)
        .map(|entry| {
            entry
                .core
                .indexes()
                .server_manifests()
                .into_iter()
                .map(|(kind, manifest)| index_manifest_listing(kind, manifest))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    medulla_protocol_types::result_contract::cluster::GraphListing {
        name: name.to_string(),
        graph_type,
        materialization: readiness
            .as_ref()
            .map(|value| materialization_phase(&value.phase)),
        source_snapshot_version: readiness
            .as_ref()
            .and_then(|value| value.source_snapshot_version),
        completeness_cursor: readiness
            .as_ref()
            .and_then(|value| value.completeness_cursor.as_ref())
            .map(|cursor| {
                medulla_protocol_types::result_contract::cluster::MaterializationCursor {
                    node_offset: cursor.node_offset as u64,
                    edge_offset: cursor.edge_offset as u64,
                }
            }),
        valid: readiness.as_ref().is_some_and(|value| value.valid),
        index_manifests,
    }
}

fn materialization_phase(
    phase: &medulla_core::registry::MaterializationPhase,
) -> medulla_protocol_types::result_contract::cluster::MaterializationPhase {
    match phase {
        medulla_core::registry::MaterializationPhase::CatalogOnly => {
            medulla_protocol_types::result_contract::cluster::MaterializationPhase::CatalogOnly
        }
        medulla_core::registry::MaterializationPhase::Partial => {
            medulla_protocol_types::result_contract::cluster::MaterializationPhase::Partial
        }
        medulla_core::registry::MaterializationPhase::Complete => {
            medulla_protocol_types::result_contract::cluster::MaterializationPhase::Complete
        }
        medulla_core::registry::MaterializationPhase::Failed => {
            medulla_protocol_types::result_contract::cluster::MaterializationPhase::Failed
        }
    }
}

fn index_manifest_listing(
    kind: medulla_graph_core::index::IndexKind,
    manifest: medulla_graph_core::index::IndexManifest,
) -> medulla_protocol_types::result_contract::cluster::IndexManifestListing {
    medulla_protocol_types::result_contract::cluster::IndexManifestListing {
        kind: index_kind_label(kind).to_string(),
        source_snapshot_version: manifest.source_snapshot_version,
        build_version: manifest.build_version,
        completeness_cursor: medulla_protocol_types::result_contract::cluster::IndexCompleteness {
            nodes: manifest.completeness.nodes,
            edges: manifest.completeness.edges,
            complete: manifest.completeness.complete,
        },
        validity: index_validity_label(manifest.validity).to_string(),
    }
}

/// The verified lifecycle identity of `req` (EH-375: tenant scope from the verified
/// carrier), or the refusal response.
fn lifecycle_request(
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
) -> Result<GraphLifecycleRequest, Response> {
    GraphLifecycleRequest::verified(
        req.id,
        req.agent_id.clone(),
        verified_context,
        verified_context.idempotency_key().to_string(),
    )
    .map_err(|denied| Response::err(req.id, denied))
}
