use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphView;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::compute::compute_off_lock;

/// Whether a SPARQL response carries per-row witness proofs (EH-197).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SparqlProofMode {
    Rows,
    RowsWithProofs,
}

impl SparqlProofMode {
    /// The mode a request's `explain` flag asks for.
    pub(super) fn of(explain: bool) -> Self {
        if explain {
            Self::RowsWithProofs
        } else {
            Self::Rows
        }
    }
}

/// Evaluate a SPARQL SELECT against the caller-visible graph snapshot, its default
/// graph unioned with the caller-filtered view of each of the tenant's visible pack
/// graphs in `packs` (MEDULLA-MEDIA-VOCAB-R655). `proj` is the
/// request's LPG→RDF projection: an empty `base_iri` keeps the identity projection; a
/// caller-supplied namespace and convention project the live property graph into
/// that vocabulary.
#[cfg(feature = "sparql")]
pub(super) async fn handle_sparql(
    req_id: u64,
    (core, packs): (Arc<GraphCore>, Vec<(String, Arc<GraphCore>)>),
    query: String,
    proj: medulla_rdf::sparql::Projection,
    mode: SparqlProofMode,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Response {
    // Each pack view is taken before the key is built, so the key names the exact
    // pack versions the evaluation reads; a pack write changes the key.
    let pack_views: Vec<(String, Arc<GraphView>, u64)> = packs
        .iter()
        .map(|(name, pack)| {
            #[cfg(feature = "result-cache")]
            let (view, version) = sparql_snapshot(
                pack,
                #[cfg(feature = "security")]
                caller,
                #[cfg(feature = "security")]
                rls,
            );
            #[cfg(not(feature = "result-cache"))]
            let (view, version) = (
                sparql_snapshot_uncached(
                    pack,
                    #[cfg(feature = "security")]
                    caller,
                    #[cfg(feature = "security")]
                    rls,
                ),
                pack.version(),
            );
            (name.clone(), view, version)
        })
        .collect();
    #[cfg(feature = "result-cache")]
    let cache_key = {
        let mut key = format!(
            "{query}\u{0}{:?}\u{0}{}\u{0}{mode:?}",
            proj.base_iri, proj.camel_type
        );
        for (name, _, version) in &pack_views {
            key.push_str(&format!("\u{0}{name}@{version}"));
        }
        key
    };
    let pack_views: Vec<Arc<GraphView>> = pack_views.into_iter().map(|(_, view, _)| view).collect();
    #[cfg(feature = "result-cache")]
    let hash = sparql_cache_hash(
        &cache_key,
        #[cfg(feature = "security")]
        caller,
    );
    #[cfg(feature = "result-cache")]
    if let Some(response) = sparql_cache_hit(&core, req_id, hash) {
        return response;
    }
    #[cfg(feature = "result-cache")]
    let (snap, version) = sparql_snapshot(
        &core,
        #[cfg(feature = "security")]
        caller,
        #[cfg(feature = "security")]
        rls,
    );
    #[cfg(not(feature = "result-cache"))]
    let snap = sparql_snapshot_uncached(
        &core,
        #[cfg(feature = "security")]
        caller,
        #[cfg(feature = "security")]
        rls,
    );
    evaluate_sparql(
        req_id,
        (snap, pack_views),
        (query, proj, mode),
        #[cfg(feature = "result-cache")]
        core,
        #[cfg(feature = "result-cache")]
        hash,
        #[cfg(feature = "result-cache")]
        version,
    )
    .await
}

#[cfg(feature = "result-cache")]
fn sparql_cache_hash(cache_key: &str, #[cfg(feature = "security")] caller: &str) -> u128 {
    #[cfg(feature = "security")]
    {
        let kind = format!("rls:{caller}:sparql");
        medulla_query_cache::result_cache::ResultCache::hash_query(&kind, cache_key.as_bytes())
    }
    #[cfg(not(feature = "security"))]
    medulla_query_cache::result_cache::ResultCache::hash_query("sparql", cache_key.as_bytes())
}

#[cfg(feature = "result-cache")]
fn sparql_cache_hit(core: &GraphCore, req_id: u64, hash: u128) -> Option<Response> {
    core.result_cache().get(hash, core.version()).map(|bytes| {
        Response::ok(
            req_id,
            ResultPayload::of_cache_hit::<medulla_protocol_types::result_contract::reasoning::Sparql>(bytes),
        )
    })
}

/// Acquire the snapshot used by the result-cache path. The whole-result probe is
/// performed by [`sparql_cache_hit`] before this per-actor filtered-view probe.
#[cfg(feature = "result-cache")]
fn sparql_snapshot(
    core: &Arc<GraphCore>,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> (Arc<GraphView>, u64) {
    #[cfg(feature = "security")]
    {
        let probe_version = core.version();
        match core.cached_filtered_view(caller, probe_version) {
            Some(cached) => (cached, probe_version),
            None => {
                let generation = core.filtered_view_cache_generation();
                let (mut snap, built_version) = core.analysis_snapshot_versioned();
                rls.filter_view(caller, &mut snap);
                let snap = Arc::new(snap);
                core.put_cached_filtered_view(
                    caller.to_string(),
                    built_version,
                    generation,
                    snap.clone(),
                );
                (snap, built_version)
            }
        }
    }
    #[cfg(not(feature = "security"))]
    core.analysis_snapshot_versioned()
}

/// Acquire a filtered snapshot when the result cache is not compiled.
#[cfg(not(feature = "result-cache"))]
fn sparql_snapshot_uncached(
    core: &Arc<GraphCore>,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Arc<GraphView> {
    #[cfg(feature = "security")]
    {
        let probe_version = core.version();
        return match core.cached_filtered_view(caller, probe_version) {
            Some(cached) => cached,
            None => {
                let generation = core.filtered_view_cache_generation();
                let mut snap = core.analysis_snapshot();
                rls.filter_view(caller, &mut snap);
                let snap = Arc::new(snap);
                core.put_cached_filtered_view(
                    caller.to_string(),
                    probe_version,
                    generation,
                    snap.clone(),
                );
                snap
            }
        };
    }
    #[cfg(not(feature = "security"))]
    Arc::new(core.analysis_snapshot())
}

async fn evaluate_sparql(
    req_id: u64,
    (snap, pack_views): (Arc<GraphView>, Vec<Arc<GraphView>>),
    (query, proj, mode): (String, medulla_rdf::sparql::Projection, SparqlProofMode),
    #[cfg(feature = "result-cache")] core: Arc<GraphCore>,
    #[cfg(feature = "result-cache")] hash: u128,
    #[cfg(feature = "result-cache")] version: u64,
) -> Response {
    match compute_off_lock(req_id, move || {
        let merged = (!pack_views.is_empty()).then(|| {
            medulla_rdf::sparql::merge_views(
                std::iter::once(&*snap).chain(pack_views.iter().map(|view| &**view)),
            )
        });
        run_sparql(
            &medulla_rdf::sparql::Dataset::new(merged.as_ref().unwrap_or(&snap), Vec::new()),
            &query,
            &proj,
            mode,
        )
    })
    .await
    {
        Ok(Ok(wire)) => {
            match ResultPayload::of_ref::<medulla_protocol_types::result_contract::reasoning::Sparql>(
                &wire,
            ) {
                Ok(payload) => {
                    #[cfg(feature = "result-cache")]
                    medulla_query_cache::result_cache::cache_result(
                        core.result_cache(),
                        hash,
                        version,
                        &payload,
                    );
                    Response::ok(req_id, payload)
                }
                Err(error) => Response::err(req_id, error),
            }
        }
        Ok(Err(msg)) => Response::err(req_id, sparql_refusal(msg)),
        Err(resp) => resp,
    }
}

/// A coded refusal raised inside evaluation (`CODE: detail`, e.g. the GeoSPARQL
/// `GEO_LITERAL_INVALID`) keeps its registered code on the wire; any other evaluation
/// error is a SPARQL error (MEDULLA-FEDERATED-QUERY-R709.4).
fn sparql_refusal(msg: String) -> String {
    let coded = msg
        .split_once(": ")
        .is_some_and(|(code, _)| medulla_protocol_types::error_registry::declared_error_code(code));
    if coded {
        msg
    } else {
        format!("SPARQL error: {msg}")
    }
}

/// Evaluate `query` and project it to the wire table, with a witness proof per row
/// when `mode` asks for one.
fn run_sparql(
    ds: &medulla_rdf::sparql::Dataset,
    query: &str,
    proj: &medulla_rdf::sparql::Projection,
    mode: SparqlProofMode,
) -> Result<medulla_protocol_types::protocol::SparqlResult, String> {
    let (table, proofs) = match mode {
        SparqlProofMode::Rows => (
            medulla_rdf::sparql::execute(ds, query, proj, None)?.into_table(),
            Vec::new(),
        ),
        SparqlProofMode::RowsWithProofs => medulla_rdf::sparql::execute_explained(ds, query, proj)?,
    };
    let (vars, rows) = table.to_rows();
    // EXPLAIN names each spatial function the query calls and whether a spatial index
    // served it (MEDULLA-FEDERATED-QUERY-R709.5).
    let spatial = match mode {
        SparqlProofMode::Rows => Vec::new(),
        SparqlProofMode::RowsWithProofs => medulla_rdf::sparql::spatial_functions(query),
    };
    Ok(medulla_protocol_types::protocol::SparqlResult {
        vars,
        rows,
        proofs,
        spatial,
    })
}
