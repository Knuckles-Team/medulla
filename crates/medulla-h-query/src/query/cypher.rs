use super::*;

/// Translate a Cypher execution's `compute_off_lock` outcome into a
/// [`Response`]: the plain (non-result-cache) reply shape both the write path
/// (`handle_cypher_write`) and the uncached read fallback
/// (`handle_cypher_read`) resolve to once the query itself has run.
#[cfg(feature = "cypher")]
fn cypher_query_response(
    req_id: u64,
    outcome: Result<Result<medulla_sql::sql::QueryResult, String>, Response>,
) -> Response {
    match outcome {
        Ok(Ok(result)) => dynamic_response::<query_results::CypherQuery, _>(req_id, &result),
        Ok(Err(msg)) => Response::err(
            req_id,
            medulla_protocol_types::error_registry::classify_refusal(
                "INVALID_ARGUMENT",
                "Cypher error: ",
                &msg,
            ),
        ),
        Err(resp) => resp,
    }
}

#[cfg(feature = "cypher")]
/// Cypher WRITE surface (CONCEPT:MEDULLA-KG.query.register-each-user-table/MEDULLA-023) — the
/// `CypherMode::Write` arm of [`handle_cypher_query`]: a `CREATE`/`MERGE`/`SET`/
/// `DELETE`/`REMOVE` statement applied to the LIVE `GraphCore` via
/// `exec_cypher_write` (native medulla-core write ops — NO DataFusion; it calls
/// `mark_dirty` once after the mutation). NOT cached, NOT RLS pre-filtered
/// (writes are graph-ACL-gated upstream — this method classified Write).
pub(crate) async fn handle_cypher_write(
    req_id: u64,
    core: Arc<GraphCore>,
    query: String,
) -> Response {
    let core_w = core.clone();
    let outcome = compute_off_lock(req_id, move || {
        medulla_cypher::cypher::exec_cypher_write(&core_w, &query)
    })
    .await;
    cypher_query_response(req_id, outcome)
}

#[cfg(feature = "cypher")]
pub(crate) async fn handle_cypher_query(
    ctx: &QueryHandlerCtx<'_>,
    query: String,
    mode: medulla_protocol_types::protocol::CypherMode,
    view: Option<String>,
) -> Result<Response, Method> {
    let req_id = ctx.req_id;
    // MEDULLA-UNIFIED-DATA-PLANE-R038.6.3: a named view routes the read through view
    // compilation before the native snapshot path. A request without `view` is unchanged.
    if let Some(name) = view {
        return Ok(handle_cypher_view(ctx, query, mode, name).await);
    }
    let validation_method = Method::CypherQuery {
        query: query.clone(),
        mode,
        view: None,
    };
    if let Err(error) = validate_cypher_mode(&validation_method) {
        return Ok(Response::err(req_id, error));
    }
    if matches!(mode, medulla_protocol_types::protocol::CypherMode::Write) {
        return Ok(handle_cypher_write(req_id, ctx.core.clone(), query).await);
    }
    Ok(handle_cypher_read(ctx, query).await)
}

/// Read through a registered graph view: compile the pattern over the view's approved
/// mappings, run it live on the sources (no copy) and answer in the Cypher result shape;
/// a leading `EXPLAIN` answers the pushed statements and mapping digest instead.
#[cfg(feature = "cypher")]
async fn handle_cypher_view(
    ctx: &QueryHandlerCtx<'_>,
    query: String,
    mode: medulla_protocol_types::protocol::CypherMode,
    name: String,
) -> Response {
    let req_id = ctx.req_id;
    if matches!(mode, medulla_protocol_types::protocol::CypherMode::Write) {
        return Response::err(
            req_id,
            medulla_cypher::cypher::ViewRefusal::ReadOnly("WRITE".to_string()).to_string(),
        );
    }
    #[cfg(all(feature = "obda", feature = "federation"))]
    {
        view_read(ctx, query, name).await
    }
    #[cfg(not(all(feature = "obda", feature = "federation")))]
    {
        let _ = (query, name);
        Response::err(
            req_id,
            "INVALID_ARGUMENT: graph views need a build with the obda and federation features"
                .to_string(),
        )
    }
}

#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
struct OneView(String, Option<medulla_types::virtual_graph_view::GraphView>);

#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
impl medulla_cypher::cypher::ViewResolver for OneView {
    fn view(&self, name: &str) -> Option<medulla_types::virtual_graph_view::GraphView> {
        (name == self.0).then(|| self.1.clone()).flatten()
    }
}

#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
async fn view_read(ctx: &QueryHandlerCtx<'_>, query: String, name: String) -> Response {
    use medulla_server_ops::api::query_txn::ForeignStateExt;
    let req_id = ctx.req_id;
    let Some(carrier) = ctx
        .read_authority
        .and_then(medulla_server_api::carrier::GraphReadAuthority::carrier)
    else {
        return Response::err(
            req_id,
            "ACCESS_DENIED: a view read needs verified tenant authority".to_string(),
        );
    };
    let (registered, persist_dir) = {
        let state = ctx.state.read().await;
        (
            state.foreign_sources().views().get(carrier, &name),
            state.persist_dir.clone(),
        )
    };
    let Some(persist_dir) = persist_dir.map(std::path::PathBuf::from) else {
        return Response::err(
            req_id,
            "owner-scoped SQL catalog requires the configured persistence directory".to_string(),
        );
    };
    let (explain, text) = medulla_cypher::cypher::split_explain(&query);
    let depth =
        usize::try_from(medulla_server_api::knob_config::knobs().federation_cypher_max_path_depth)
            .unwrap_or(usize::MAX);
    let resolver = OneView(
        name.clone(),
        registered.as_ref().map(|entry| entry.view.clone()),
    );
    let plan = match medulla_cypher::cypher::resolve_view_plan(&resolver, &name, text, depth) {
        Ok(plan) => plan,
        Err(refusal) => return Response::err(req_id, refusal.to_string()),
    };
    let Some(registered) = registered else {
        return Response::err(req_id, format!("UNKNOWN_VIEW: {name}"));
    };
    let authority = carrier.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let run = medulla_h_sparql::rdf::ViewRun {
            mapping: &registered.mapping,
            tables: &registered.tables,
            sources: &registered.sources,
            plan: &plan,
        };
        medulla_h_sparql::rdf::view_virtual(&authority, &persist_dir, &run)
            .map(|answer| (answer, plan.clone()))
    })
    .await;
    let (answer, plan) = match joined {
        Ok(Ok(done)) => done,
        Ok(Err(message)) => {
            return Response::err(
                req_id,
                medulla_protocol_types::error_registry::classify_refusal(
                    "INVALID_ARGUMENT",
                    "Cypher view error: ",
                    &message,
                ),
            )
        }
        Err(error) => {
            return Response::err(req_id, format!("Cypher view task join error: {error}"))
        }
    };
    let result = if explain {
        view_explain_result(&plan, &answer.pushed)
    } else {
        view_rows_result(&plan, &answer)
    };
    match result {
        Ok(result) => dynamic_response::<query_results::CypherQuery, _>(req_id, &result),
        Err(message) => Response::err(
            req_id,
            format!("INVALID_ARGUMENT: Cypher view error: {message}"),
        ),
    }
}

/// The answer in the Cypher result shape: one column per RETURN item, cells in the
/// lexical form the source returned (numbers and booleans keep their JSON type).
#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
fn view_rows_result(
    plan: &medulla_types::virtual_graph_view::ViewPlan,
    answer: &medulla_h_sparql::rdf::ViewAnswer,
) -> Result<medulla_protocol_types::protocol::QueryResult, String> {
    let index: Vec<Option<usize>> = plan
        .projection
        .iter()
        .map(|(variable, _)| answer.vars.iter().position(|v| v == variable))
        .collect();
    let columns: Vec<String> = plan
        .projection
        .iter()
        .map(|(_, column)| column.clone())
        .collect();
    let mut rows = Vec::with_capacity(answer.rows.len());
    for row in &answer.rows {
        let cells: Vec<serde_json::Value> = index
            .iter()
            .map(
                |at| match at.and_then(|i| row.get(i)).and_then(Option::as_ref) {
                    Some(text) => lexical_cell(text),
                    None => serde_json::Value::Null,
                },
            )
            .collect();
        rows.push(rmp_serde::to_vec(&cells).map_err(|e| format!("encode row: {e}"))?);
    }
    Ok(medulla_protocol_types::protocol::QueryResult { columns, rows })
}

#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
fn lexical_cell(text: &str) -> serde_json::Value {
    if let Ok(number) = text.parse::<i64>() {
        return serde_json::Value::from(number);
    }
    if let Some(number) = text
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
    {
        return serde_json::Value::Number(number);
    }
    serde_json::Value::String(text.to_string())
}

/// `EXPLAIN`: one row per fragment with its pushed statement and mapping digest, built from
/// the same [`FragmentTrace`] the federated profile renders.
#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
fn view_explain_result(
    plan: &medulla_types::virtual_graph_view::ViewPlan,
    pushed: &[(String, String)],
) -> Result<medulla_protocol_types::protocol::QueryResult, String> {
    use medulla_federation::federation_opt::{
        render_trace, FetchStrategy, FragmentExecutionMode, FragmentTrace,
    };
    let columns = vec![
        "fragment".to_string(),
        "pushed_statement".to_string(),
        "mapping_digest".to_string(),
    ];
    let mut rows = Vec::new();
    for fragment in medulla_cypher::cypher::explain_fragments(plan, pushed) {
        let mut trace = FragmentTrace::new(
            format!(
                "sql:{}#{}",
                fragment.source,
                source_fingerprint(&fragment.source)
            ),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Live,
        );
        trace.requests = 1;
        trace.pushed_statement = Some(fragment.pushed_statement.clone());
        trace.mapping_digest = Some(fragment.mapping_digest.clone());
        let cells = vec![
            serde_json::Value::String(render_trace(&[trace])),
            serde_json::Value::String(fragment.pushed_statement),
            serde_json::Value::String(fragment.mapping_digest),
        ];
        rows.push(rmp_serde::to_vec(&cells).map_err(|e| format!("encode row: {e}"))?);
    }
    Ok(medulla_protocol_types::protocol::QueryResult { columns, rows })
}

/// The 8-hex fingerprint of a source name, the label shape `FragmentTrace` expects.
#[cfg(all(feature = "cypher", feature = "obda", feature = "federation"))]
fn source_fingerprint(name: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(name.as_bytes())[..4])
}

#[cfg(feature = "cypher")]
async fn handle_cypher_read(ctx: &QueryHandlerCtx<'_>, query: String) -> Response {
    let req_id = ctx.req_id;
    let caller = ctx.caller;
    let core = ctx.core.clone();
    #[cfg(feature = "security")]
    let rls = ctx.rls;
    // Same off-lock snapshot + blocking-pool idiom as SQL — but DEP-FREE
    // (label index / VF2 / BFS), so it runs in a no-DataFusion Pi build.
    // Version-keyed, RLS-aware result cache (CONCEPT:MEDULLA-KG.coordination.distributed-cache-coherence × KG-2.231) wraps
    // it identically; this is the lean-Pi cached query path. The cache KEY folds
    // in the caller's RLS context so agent A's filtered rows are never served to
    // agent B, and the snapshot is RLS-filtered before execution.
    #[cfg(feature = "result-cache")]
    let (snap, version, hash) = {
        let hash = rls_cache_hash(
            "cypher",
            query.as_bytes(),
            #[cfg(feature = "security")]
            caller,
            #[cfg(feature = "security")]
            rls,
        );
        // BUG-267: `analysis_snapshot()` was materialized unconditionally
        // BEFORE this cache lookup — an O(V+E) clone of the entire
        // node_map/node_properties/edge_properties paid on every call
        // regardless of cache hit or miss, defeating the cache that
        // follows it (measured p99 2.49s on plain cached reads). Probe
        // with the cheap `core.version()` atomic load first; only a MISS
        // pays for `analysis_snapshot_versioned()`. The fresh `version`
        // it returns (not the one used for this probe) is what `put`
        // stores under below, so an entry can never claim a version
        // newer than the data it reflects.
        //
        // BUG-267 follow-up (regression caught by
        // `result_cache_dispatch_tests::hit_on_unchanged_then_write_invalidates`
        // and `rls_aware_cache_no_cross_agent_leak::
        // agent_a_cached_result_is_not_served_to_agent_b`, both of which assert
        // an EXACT miss delta of 1 per served request): an earlier version of
        // this fix re-checked the cache a SECOND time after the snapshot too, to
        // guard a concurrent-populate race. `ResultCache::get` bumps the
        // hit/miss counters on EVERY call — including a probe that finds
        // nothing — so that second check silently double-counted every miss.
        // There is only ONE counted lookup on this path now; a rare concurrent
        // populate between the probe and the snapshot just costs a redundant
        // recompute (the `put` below still lands under this call's own fresh
        // `version`, so nothing stale or cross-actor is ever served).
        if let Some(bytes) = core.result_cache().get(hash, core.version()) {
            return Response::ok(
                req_id,
                ResultPayload::of_encoded::<query_results::CypherQuery>(bytes),
            );
        }
        // perf/cold-query-floor-analysis (UNCOMPILED PROPOSAL — see
        // `crate::rls_view_cache` in medulla-core, not yet exercised by any test or
        // build): a whole-RESULT-cache MISS used to unconditionally pay for
        // `analysis_snapshot_versioned()` (an O(V+E) clone of every node/edge
        // property blob's Arc handle) PLUS `rls.filter_view` — which, per node
        // in the ENTIRE snapshot (not just the rows this query's WHERE clause
        // ultimately matches), fully msgpack-decodes that node's property blob
        // (`IsolationLayer::can_see_node` -> `row_visibility`) just to read 2-3
        // small RLS metadata keys. That is architecturally the SAME bug
        // `build_cypher_label_index` had before `perf/warm-label-index`
        // (`2662713b`) fixed it for the label index, except unmemoized: this
        // filtered view is a pure function of (graph content, actor's ACL
        // grants) at one `version()`, so a same-(actor, version) repeat pays the
        // full O(V) decode again on every distinct query text (a whole-result
        // cache miss), which is exactly the reported ~900ms fixed floor —
        // measured live via the structurally identical `project_core` cache's
        // own `medulla_projection_cache_miss_build_seconds` (298
        // misses, mean 1.10s/miss). Probe the per-actor filtered-view cache
        // FIRST; only a genuine cold (actor, version) pair pays for the
        // snapshot + filter, exactly mirroring the `result_cache` probe just
        // above (BUG-267) and `GraphReadAuthority::project_core`'s existing
        // cache-then-build shape (`src/server/access.rs`). RLS safety: keyed by
        // (actor, version) exactly like `project_core`'s cache — never a
        // cross-actor share — and invalidated on every whole-image transition
        // `project_core`'s cache is (`GraphCore::invalidate_filtered_view_cache`,
        // called alongside `invalidate_projection_cache` at both its sites).
        // `version` MUST be the exact version the returned `snap` reflects (the
        // same BUG-267 invariant the `result_cache.put` below relies on: an
        // entry can never claim a version newer than the data it reflects) —
        // NOT a fresh `core.version()` re-read after the cache probe, which
        // could have raced a concurrent commit. The hit branch reuses
        // `probe_version` (what `cached_filtered_view` matched against); the
        // miss branch reuses `built_version` (what `analysis_snapshot_versioned`
        // itself captured), exactly as the pre-existing code did.
        #[cfg(feature = "security")]
        let (snap, version): (Arc<medulla_graph_core::graph::GraphView>, u64) =
            versioned_rls_snapshot(&core, caller, rls);
        #[cfg(not(feature = "security"))]
        let (snap, version) = core.analysis_snapshot_versioned();
        (snap, version, hash)
    };
    #[cfg(not(feature = "result-cache"))]
    let snap = rls_snapshot(
        &core,
        #[cfg(feature = "security")]
        caller,
        #[cfg(feature = "security")]
        rls,
    );
    // `version`-paired snapshot (`result-cache` build only, see above): hand
    // `exec_cypher_params_indexed` an `IndexSource` so an unlabeled `MATCH (n)
    // WHERE n.id = …`/`IN […]` — the fleet/tool-registration slow-query shape —
    // can narrow through the bounded property index instead of a whole-graph
    // scan (CONCEPT:MEDULLA-KG.storage.index-manager-seam). The index answers off `core`'s LIVE
    // state, so `exec_cypher_params_indexed` brackets it against `core.version()`
    // and discards the answer (full-scan fallback) if a concurrent commit raced
    // this snapshot; the SERVED result is identical to plain `exec_cypher` either
    // way — only the work to reach it differs.
    #[cfg(feature = "result-cache")]
    let core_for_index = core.clone();
    #[cfg(feature = "result-cache")]
    let resp = match compute_off_lock(req_id, move || {
        medulla_cypher::cypher::exec_cypher_params_indexed(
            &snap,
            &query,
            &medulla_cypher::cypher::Params::new(),
            medulla_cypher::cypher::IndexSource::new(&core_for_index, version),
        )
    })
    .await
    {
        Ok(Ok(result)) => match ResultPayload::of_dynamic::<query_results::CypherQuery, _>(&result)
        {
            Ok(payload) => cache_and_respond(&core, req_id, hash, version, payload),
            Err(error) => Response::err(req_id, error),
        },
        Ok(Err(msg)) => Response::err(
            req_id,
            medulla_protocol_types::error_registry::classify_refusal(
                "INVALID_ARGUMENT",
                "Cypher error: ",
                &msg,
            ),
        ),
        Err(resp) => resp,
    };
    #[cfg(not(feature = "result-cache"))]
    let resp = cypher_query_response(
        req_id,
        compute_off_lock(req_id, move || {
            medulla_cypher::cypher::exec_cypher(&snap, &query)
        })
        .await,
    );
    resp
}
