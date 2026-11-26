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

/// The shape policies a SPARQL read is held to, and where the record of their evaluation goes
/// (MEDULLA-FEDERATED-QUERY-R706.12).
pub(super) struct SparqlShapeGate {
    pub(super) policies: medulla_exec::policy_mask::PolicySet,
    pub(super) principal: String,
    pub(super) log: Arc<medulla_exec::policy_mask::PolicyEvaluationLog>,
}

/// The graph node a bound cell names, if any: the cell itself, the cell without its angle
/// brackets, or the node a projected IRI of the core ontology namespace stands for.
fn node_of_cell(view: &GraphView, cell: &str) -> Option<String> {
    let bare = cell
        .strip_prefix('<')
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or(cell);
    let local = bare.strip_prefix(medulla_types::ontology_base::CORE_NAMESPACE);
    [
        Some(cell.to_string()),
        Some(bare.to_string()),
        Some(format!("<{bare}>")),
        local.map(str::to_string),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| view.node_properties.contains_key(candidate.as_str()))
}

/// Hold a SPARQL result to the shape policies (MEDULLA-FEDERATED-QUERY-R706.12).
///
/// A SPARQL solution's nodes are every graph node bound in it: each bound IRI that names a
/// node of `view`. A solution is filtered, or the request refused, when any of its nodes
/// violates a shape; a solution that binds no node (only literals) is not shape-checked. The
/// witness proofs of the rows that stay are renumbered to their new row. The evaluation is
/// recorded once, answered or refused.
fn apply_sparql_shapes(
    wire: &mut medulla_protocol_types::protocol::SparqlResult,
    gate: &SparqlShapeGate,
    view: &GraphView,
) -> Result<(), String> {
    use medulla_exec::policy_mask::PolicyAuditSink as _;
    let row_nodes: Vec<Vec<String>> = wire
        .rows
        .iter()
        .map(|row| {
            let mut nodes: Vec<String> = row
                .iter()
                .flatten()
                .filter_map(|cell| node_of_cell(view, cell))
                .collect();
            nodes.sort();
            nodes.dedup();
            nodes
        })
        .collect();
    let verdicts = medulla_exec::policy_mask::judge_rows(&gate.policies, view, &row_nodes)?;
    gate.log.record(medulla_exec::policy_mask::PolicyEvaluation {
        policy_digest: medulla_exec::policy_mask::policy_digest(&gate.policies),
        principal: gate.principal.clone(),
        counts: verdicts.counts,
    });
    if let Some(policy_id) = verdicts.violated_policy {
        return Err(medulla_exec::policy_mask::violation_refusal(&policy_id));
    }
    // Old row index -> new row index of the rows that stay.
    let mut renumbered: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let mut next = 0_u64;
    for (old, kept) in verdicts.keep.iter().enumerate() {
        if *kept {
            renumbered.insert(old as u64, next);
            next += 1;
        }
    }
    let mut keep = verdicts.keep.iter();
    wire.rows.retain(|_| keep.next().copied().unwrap_or(true));
    wire.proofs.retain_mut(|proof| match renumbered.get(&proof.row) {
        Some(new) => {
            proof.row = *new;
            true
        }
        None => false,
    });
    Ok(())
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
    max_solutions: usize,
    graph_media_type: Option<&'static str>,
    shapes: Option<SparqlShapeGate>,
    #[cfg(feature = "security")] caller: &str,
    #[cfg(feature = "security")] rls: &Arc<medulla_isolation::isolation::IsolationLayer>,
) -> Response {
    // The request graph's runtime-imported ontologies are read the same way as a pack
    // graph: their classes and properties join the default graph, so a served query
    // sees an imported class after its revision and not after its withdrawal
    // (MEDULLA-TYPED-PACKS-R103.6). The composed digest joins the cache key.
    let ontology = match runtime_ontology_view(&core) {
        Ok(ontology) => ontology,
        Err(error) => return Response::err(req_id, format!("SPARQL error: {error}")),
    };
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
            "{query}\u{0}{:?}\u{0}{}\u{0}{mode:?}\u{0}{graph_media_type:?}",
            proj.base_iri, proj.camel_type
        );
        if let Some(usage) = &proj.usage {
            let mut entries: Vec<_> = usage.iter().collect();
            entries.sort();
            key.push_str(&format!("\u{0}usage@{entries:?}"));
        }
        for (name, _, version) in &pack_views {
            key.push_str(&format!("\u{0}{name}@{version}"));
        }
        key.push_str(&format!("\u{0}ontology@{}", ontology.digest));
        key
    };
    let mut pack_views: Vec<Arc<GraphView>> =
        pack_views.into_iter().map(|(_, view, _)| view).collect();
    pack_views.extend(ontology.view);
    #[cfg(feature = "result-cache")]
    let hash = sparql_cache_hash(
        &cache_key,
        #[cfg(feature = "security")]
        caller,
    );
    // MEDULLA-FEDERATED-QUERY-R706.12: a request under shape policies is evaluated, so its
    // record is owed and its answer is never cached or served from the cache.
    #[cfg(feature = "result-cache")]
    if shapes.is_none() {
        if let Some(response) = sparql_cache_hit(&core, req_id, hash) {
            return response;
        }
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
        (query, proj, mode, max_solutions, graph_media_type),
        shapes,
        #[cfg(feature = "result-cache")]
        core,
        #[cfg(feature = "result-cache")]
        hash,
        #[cfg(feature = "result-cache")]
        version,
    )
    .await
}

/// The runtime ontologies of one graph as a readable view, with the composed digest
/// that names exactly which revisions it holds.
struct RuntimeOntology {
    view: Option<Arc<GraphView>>,
    #[cfg_attr(not(feature = "result-cache"), allow(dead_code))]
    digest: String,
}

/// Build the view of every `SchemaSourceOrigin::Ontology` source composed into
/// `core`'s GraphSchema: the active revision's ontology Turtle lowered to graph rows
/// with the same lowering a bulk RDF load uses. Engine core vocabulary and attached
/// packs stay out: only a connector's imported ontology joins the default graph. The
/// view is derived from the composed sources on every read, so it never outlives a
/// revision or a withdrawal.
fn runtime_ontology_view(core: &GraphCore) -> Result<RuntimeOntology, String> {
    use medulla_graph_core::graph::SchemaSourceOrigin;
    let sources = core.schema_sources();
    let digest = sources.composed_digest().to_hex();
    let mut triples = Vec::new();
    for (id, source) in sources.all() {
        if !matches!(source.origin, SchemaSourceOrigin::Ontology { .. }) {
            continue;
        }
        let Some(document) = source.ontology_ttl.as_deref() else {
            continue;
        };
        let parsed = medulla_rdf::mapping::parse_turtle(document)
            .map_err(|error| format!("ontology source '{id}': {error}"))?;
        triples.extend(parsed);
    }
    if triples.is_empty() {
        return Ok(RuntimeOntology { view: None, digest });
    }
    let scratch = GraphCore::new();
    let mut iris = medulla_rdf::mapping::IriStore::default();
    medulla_rdf::mapping::load_triples(&scratch, &mut iris, "runtime-ontology", triples)?;
    Ok(RuntimeOntology {
        view: Some(Arc::new(scratch.analysis_snapshot())),
        digest,
    })
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
    (query, proj, mode, max_solutions, graph_media_type): (
        String,
        medulla_rdf::sparql::Projection,
        SparqlProofMode,
        usize,
        Option<&'static str>,
    ),
    shapes: Option<SparqlShapeGate>,
    #[cfg(feature = "result-cache")] core: Arc<GraphCore>,
    #[cfg(feature = "result-cache")] hash: u128,
    #[cfg(feature = "result-cache")] version: u64,
) -> Response {
    #[cfg(feature = "result-cache")]
    let has_shapes = shapes.is_some();
    match compute_off_lock(req_id, move || {
        let merged = (!pack_views.is_empty()).then(|| {
            medulla_rdf::sparql::merge_views(
                std::iter::once(&*snap).chain(pack_views.iter().map(|view| &**view)),
            )
        });
        let view = merged.as_ref().unwrap_or(&snap);
        let mut wire = run_sparql(
            &medulla_rdf::sparql::Dataset::new(view, Vec::new()),
            &query,
            &proj,
            mode,
            (max_solutions, graph_media_type),
        )?;
        if let Some(gate) = &shapes {
            apply_sparql_shapes(&mut wire, gate, view)?;
        }
        Ok::<_, String>(wire)
    })
    .await
    {
        Ok(Ok(wire)) => {
            match ResultPayload::of_ref::<medulla_protocol_types::result_contract::reasoning::Sparql>(
                &wire,
            ) {
                Ok(payload) => {
                    #[cfg(feature = "result-cache")]
                    if !has_shapes {
                        medulla_query_cache::result_cache::cache_result(
                            core.result_cache(),
                            hash,
                            version,
                            &payload,
                        );
                    }
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

/// The graph of a `CONSTRUCT` or `DESCRIBE` outcome serialized in `media_type`; `None` for any
/// other outcome or when no media type was asked for.
fn graph_body_of(
    outcome: &medulla_rdf::sparql::QueryOutcome,
    media_type: Option<&'static str>,
) -> Result<Option<String>, String> {
    match (outcome, media_type) {
        (medulla_rdf::sparql::QueryOutcome::Graph(triples), Some(media_type)) => {
            medulla_rdf::graph_media::serialize_graph(media_type, triples).map(Some)
        }
        _ => Ok(None),
    }
}

/// Evaluate `query` and project it to the wire table, with a witness proof per row
/// when `mode` asks for one. A `CONSTRUCT` or `DESCRIBE` answer also carries its graph
/// serialized in `graph_media_type` when the request named one (MEDULLA-FEDERATED-QUERY-R740.2).
fn run_sparql(
    ds: &medulla_rdf::sparql::Dataset,
    query: &str,
    proj: &medulla_rdf::sparql::Projection,
    mode: SparqlProofMode,
    (max_solutions, graph_media_type): (usize, Option<&'static str>),
) -> Result<medulla_protocol_types::protocol::SparqlResult, String> {
    let (table, proofs, graph_body) = match mode {
        SparqlProofMode::Rows => {
            let outcome = medulla_rdf::sparql::execute(ds, query, proj, None)?;
            let body = graph_body_of(&outcome, graph_media_type)?;
            (outcome.into_table(), Vec::new(), body)
        }
        SparqlProofMode::RowsWithProofs => {
            let (table, proofs) = medulla_rdf::sparql::execute_explained(ds, query, proj)?;
            let body = match graph_media_type {
                Some(_) => graph_body_of(
                    &medulla_rdf::sparql::execute(ds, query, proj, None)?,
                    graph_media_type,
                )?,
                None => None,
            };
            (table, proofs, body)
        }
    };
    let (vars, rows) = table.to_rows();
    // MEDULLA-FEDERATED-QUERY-R731: the interactive meter is the solution count before
    // projection; past the bound the query refuses and names the remedy.
    medulla_exec::budget::Budget {
        max_interactive_nodes: Some(max_solutions),
        ..medulla_exec::budget::Budget::default()
    }
    .check_interactive(rows.len())?;
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
        graph_body,
    })
}
