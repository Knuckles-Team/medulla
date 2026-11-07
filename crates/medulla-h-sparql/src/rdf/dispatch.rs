use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::api::reentry::TryHandleContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;

/// Route the native RDF family while keeping each protocol surface in its own
/// cohesive handler. An unmatched method is returned for the next dispatcher.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    ctx: TryHandleContext<'_>,
    core: Arc<GraphCore>,
    method: Method,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Result<Response, Method> {
    let TryHandleContext {
        req_id,
        graph_name,
        read_authority,
        caller,
    } = ctx;

    if let Some(response) =
        try_handle_rdf(state, req_id, graph_name, read_authority, &core, &method).await
    {
        return Ok(response);
    }
    #[cfg(feature = "sparql")]
    if let Some(response) = try_handle_sparql(
        state,
        req_id,
        (graph_name, read_authority),
        core.clone(),
        &method,
        #[cfg(feature = "security")]
        caller,
        #[cfg(feature = "security")]
        rls,
    )
    .await
    {
        return Ok(response);
    }
    #[cfg(feature = "owl")]
    if let Some(response) = try_handle_owl(req_id, read_authority, core.clone(), &method).await {
        return Ok(response);
    }
    #[cfg(feature = "obda")]
    if let Some(response) = try_handle_obda(state, req_id, read_authority, &method).await {
        return Ok(response);
    }
    if let Some(response) = try_handle_rules(
        req_id,
        &core,
        &method,
        #[cfg(feature = "security")]
        caller,
        #[cfg(feature = "security")]
        rls,
    )
    .await
    {
        return Ok(response);
    }
    #[cfg(any(feature = "shacl", feature = "shex"))]
    if let Some(response) =
        try_handle_validation(req_id, graph_name, read_authority, &core, &method).await
    {
        return Ok(response);
    }
    Err(method)
}

async fn try_handle_rdf(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
    core: &Arc<GraphCore>,
    method: &Method,
) -> Option<Response> {
    match method {
        Method::AddTriples { turtle, ntriples } => Some(
            super::triples::handle_add_triples(
                req_id,
                graph_name,
                core,
                turtle.clone(),
                ntriples.clone(),
            )
            .await,
        ),
        Method::GetRdf => {
            let authority =
                read_authority.expect("GetRdf must carry the universal served-read authority");
            let projected = authority.project_core(core);
            Some(super::triples::handle_get_rdf(req_id, graph_name, &projected).await)
        }
        Method::RemoveTriples { turtle, ntriples } => Some(
            super::triples::handle_remove_triples(
                #[cfg(feature = "shacl")]
                state,
                req_id,
                graph_name,
                core,
                turtle.clone(),
                ntriples.clone(),
            )
            .await,
        ),
        Method::DropNamedGraph => {
            Some(super::triples::handle_drop_named_graph(req_id, graph_name, core).await)
        }
        _ => None,
    }
}

#[cfg(feature = "sparql")]
async fn try_handle_sparql(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    (graph_name, read_authority): (&str, Option<&GraphReadAuthority>),
    core: Arc<GraphCore>,
    method: &Method,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Option<Response> {
    match method {
        Method::Sparql {
            query,
            base_iri,
            type_convention,
            explain,
        } => Some(
            match visible_pack_graphs(state, graph_name, read_authority).await {
                Err(error) => Response::err(req_id, error),
                Ok(packs) => {
                    super::sparql::handle_sparql(
                        req_id,
                        (core, packs),
                        query.clone(),
                        medulla_rdf::sparql::Projection::from_wire(base_iri, type_convention),
                        super::sparql::SparqlProofMode::of(*explain),
                        #[cfg(feature = "security")]
                        caller,
                        #[cfg(feature = "security")]
                        rls,
                    )
                    .await
                }
            },
        ),
        _ => None,
    }
}

/// The resident pack graphs of the caller's verified tenant, which the served
/// SPARQL default graph unions (MEDULLA-MEDIA-VOCAB-R655): a connector pack's
/// ontology individuals live in its own `pack__<digest>` graph, so a query on any
/// other graph sees them only through this union. A caller without verified
/// tenant authority, or a deployment without a persistence directory, sees none;
/// an Agent Library that cannot be read refuses the query.
#[cfg(feature = "sparql")]
async fn visible_pack_graphs(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
) -> Result<Vec<(String, Arc<GraphCore>)>, String> {
    #[cfg(feature = "redb")]
    {
        let Some(carrier) = read_authority.and_then(GraphReadAuthority::carrier) else {
            return Ok(Vec::new());
        };
        let tenant = carrier.log_owner().0;
        let state = state.read().await;
        let Some(persist_dir) = state.persist_dir.as_deref() else {
            return Ok(Vec::new());
        };
        let names = medulla_persistence_records::agent_library::open_shared_agent_library(
            &state.agent_library_open,
            persist_dir,
        )
        .and_then(|store| store.visible_connector_pack_graphs(tenant))?;
        Ok(names
            .into_iter()
            .filter(|name| name != graph_name)
            .filter_map(|name| {
                let core = state.registry.get(&name)?.core.clone();
                Some((name, core))
            })
            .collect())
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, graph_name, read_authority);
        Ok(Vec::new())
    }
}

#[cfg(feature = "owl")]
async fn try_handle_owl(
    req_id: u64,
    read_authority: Option<&GraphReadAuthority>,
    core: Arc<GraphCore>,
    method: &Method,
) -> Option<Response> {
    match method {
        Method::OwlReason {
            ontology,
            target_class,
            class_base,
            min_confidence,
        } => {
            let authority =
                read_authority.expect("OwlReason must carry the universal served-read authority");
            let projected = authority.project_core(&core);
            Some(
                super::reasoning::handle_owl_reason(
                    req_id,
                    &projected,
                    ontology.clone(),
                    target_class.clone(),
                    class_base.clone(),
                    *min_confidence,
                )
                .await,
            )
        }
        Method::OwlExplain { ontology, sub, sup } => {
            let authority =
                read_authority.expect("OwlExplain must carry the universal served-read authority");
            let projected = authority.project_core(&core);
            Some(
                super::reasoning::handle_owl_explain(
                    req_id,
                    &projected,
                    ontology.clone(),
                    sub.clone(),
                    sup.clone(),
                )
                .await,
            )
        }
        _ => None,
    }
}

#[cfg(feature = "obda")]
async fn try_handle_obda(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    read_authority: Option<&GraphReadAuthority>,
    method: &Method,
) -> Option<Response> {
    let Method::SparqlVirtual {
        query,
        mapping,
        tables,
        external_sources,
    } = method
    else {
        return None;
    };
    let authority = read_authority
        .and_then(GraphReadAuthority::carrier)
        .expect("SparqlVirtual must carry verified tenant authority");
    let persist_dir = match state.read().await.persist_dir.clone() {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            return Some(Response::err(
                req_id,
                "owner-scoped SQL catalog requires the configured persistence directory"
                    .to_string(),
            ));
        }
    };
    Some(
        super::obda::handle_sparql_virtual(
            req_id,
            authority.clone(),
            persist_dir,
            query.clone(),
            mapping.clone(),
            tables.clone(),
            external_sources.clone(),
        )
        .await,
    )
}

async fn try_handle_rules(
    req_id: u64,
    core: &Arc<GraphCore>,
    method: &Method,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Option<Response> {
    let Method::RunRules {
        ontology_ttl,
        rules,
        query_predicate,
        min_confidence,
        derived_only,
        explain,
        max_steps,
    } = method
    else {
        return None;
    };
    Some(
        super::reasoning::handle_run_rules(super::reasoning::RunRulesRequest {
            req_id,
            core,
            ontology_ttl: ontology_ttl.clone(),
            rules: rules.clone(),
            query_predicate: query_predicate.clone(),
            min_confidence: *min_confidence,
            derived_only: *derived_only,
            explain: *explain,
            max_steps: *max_steps,
            #[cfg(feature = "security")]
            caller,
            #[cfg(feature = "security")]
            rls,
        })
        .await,
    )
}

#[cfg(any(feature = "shacl", feature = "shex"))]
async fn try_handle_validation(
    req_id: u64,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
    core: &Arc<GraphCore>,
    method: &Method,
) -> Option<Response> {
    match method {
        #[cfg(feature = "shacl")]
        Method::ShaclValidate {
            shapes,
            data_graph,
            data_triples,
        } => {
            let authority = read_authority
                .expect("ShaclValidate must carry the universal served-read authority");
            let projected = authority.project_core(core);
            let data = super::validation::ShaclData {
                turtle: data_graph,
                triples: data_triples.as_deref(),
            };
            Some(
                super::validation::handle_shacl_validate(
                    req_id,
                    graph_name,
                    &projected,
                    shapes.clone(),
                    data,
                )
                .await,
            )
        }
        #[cfg(feature = "rdf")]
        Method::OntologyCanonicalHash { turtle } => {
            Some(super::ontology_hash::handle_ontology_canonical_hash(req_id, turtle))
        }
        #[cfg(feature = "shacl")]
        Method::OntologyInspect {
            documents,
            source_ids,
        } => Some(super::inspect::handle_ontology_inspect(
            req_id,
            core,
            documents,
            source_ids,
        )),
        #[cfg(feature = "shacl")]
        Method::GraphSchemaList => Some(
            medulla_server_ops::api::admin::schema_read::handle_list(req_id, graph_name, core).await,
        ),
        #[cfg(feature = "shacl")]
        Method::GraphSchemaClasses {
            kind,
            cursor,
            limit,
        } => Some(
            medulla_server_ops::api::admin::terms::handle_classes(
                req_id,
                graph_name,
                core,
                medulla_server_ops::api::admin::terms::TermPageRequest {
                    kind: *kind,
                    cursor: cursor.as_deref(),
                    limit: *limit,
                },
            )
            .await,
        ),
        #[cfg(feature = "shacl")]
        Method::IcvConfigure { .. } => unreachable!(
            "IcvConfigure is mutation::GATEWAY_ROUTED; dispatch_graph_op must route it through try_handle_gateway before it ever reaches this fallback handler"
        ),
        _ => try_handle_shex_validation(req_id, graph_name, read_authority, core, method).await,
    }
}

/// The ShEx half of the validation route. Its own handler because ShEx is a
/// separate schema language behind its own feature, and the two halves share
/// nothing but the projected core they read.
#[cfg(any(feature = "shacl", feature = "shex"))]
async fn try_handle_shex_validation(
    req_id: u64,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
    core: &Arc<GraphCore>,
    method: &Method,
) -> Option<Response> {
    match method {
        #[cfg(feature = "shex")]
        Method::ShexValidate {
            schema,
            data_graph,
            shape_map,
        } => {
            let authority = read_authority
                .expect("ShexValidate must carry the universal served-read authority");
            let projected = authority.project_core(core);
            Some(
                super::validation::handle_shex_validate(
                    req_id,
                    graph_name,
                    &projected,
                    schema.clone(),
                    data_graph.clone(),
                    shape_map.clone(),
                )
                .await,
            )
        }
        _ => None,
    }
}
