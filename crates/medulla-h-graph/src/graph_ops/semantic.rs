use super::*;

use super::terminal::GraphOpsContext;

const DISCOVER_FANOUT: usize = 4;
const DISCOVER_LEX_SCAN_CAP: usize = 4096;
const DISCOVER_SEM_WEIGHT: f32 = 0.6;
const DISCOVER_KW_WEIGHT: f32 = 0.4;

/// Read a node's human-readable `(name, description, type)` triple from its
/// MessagePack property blob (CONCEPT:MEDULLA-KG.retrieval.one-round-trip-discovery), used to hydrate `Discover` hits.
/// `name` falls back to the node id, `type` falls back to a `node_type` field, and
/// a missing/undecodable blob yields `(id, "", "")` — so the op never fails on a
/// text-less node, it just returns what it can.
fn node_text(core: &GraphCore, node_id: &str) -> (String, String, String) {
    let Some(blob) = core.get_node_properties(node_id) else {
        return (node_id.to_string(), String::new(), String::new());
    };
    let obj = super::gateway_graph::decode_json_object(&blob);
    let get = |k: &str| {
        obj.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let name = {
        let n = get("name");
        if n.is_empty() {
            node_id.to_string()
        } else {
            n
        }
    };
    let ntype = {
        let t = get("type");
        if t.is_empty() {
            get("node_type")
        } else {
            t
        }
    };
    (name, get("description"), ntype)
}

/// The web provenance of a node (MEDULLA-FEDERATED-QUERY-R653.7); empty unless the node has a URL.
fn web_provenance(core: &GraphCore, node_id: &str) -> medulla_types::web_source::WebProvenance {
    core.get_node_properties(node_id)
        .map(|blob| {
            medulla_types::web_source::WebProvenance::from_properties(
                &super::gateway_graph::decode_json_object(&blob),
            )
        })
        .unwrap_or_default()
}

/// Fraction of the distinct query keywords that appear (as a substring, case-insensitively)
/// anywhere in a candidate's `name`/`description`/`type` — a `[0.0, 1.0]` lexical score.
fn keyword_overlap(kws: &[String], name: &str, description: &str, ntype: &str) -> f32 {
    if kws.is_empty() {
        return 0.0;
    }
    let haystack = format!(
        "{} {} {}",
        name.to_lowercase(),
        description.to_lowercase(),
        ntype.to_lowercase()
    );
    let matched = kws.iter().filter(|kw| haystack.contains(*kw)).count();
    matched as f32 / kws.len() as f32
}

fn normalized_keywords(keywords: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    keywords
        .iter()
        .map(|word| word.to_lowercase())
        .filter(|word| !word.is_empty() && seen.insert(word.clone()))
        .collect()
}

/// The retrieval-quality verdict of one `Discover` answer (MEDULLA-FEDERATED-QUERY-R089.1).
///
/// Quality is the mean hit score and population is the returned-to-requested hit
/// ratio, both in Q16 and judged against the registry knobs
/// `discover.min_composite_quality` and `discover.min_population_ratio`. An answer
/// below a threshold names its closed failure mode; an answer below the
/// `retrieval.min_evidence` row count is a sparse index. The index generation is the
/// resident semantic index's.
fn discover_verdict(
    core: &GraphCore,
    hits: &[medulla_types::ingestion_wire::DiscoverHit],
    k: usize,
    has_emb: bool,
) -> medulla_types::wire::RetrievalVerdict {
    use medulla_types::wire::{DiscoverThresholds, RetrievalFailureMode, RetrievalVerdict};
    fn q16(x: f64) -> u16 {
        (x.clamp(0.0, 1.0) * 65_535.0).round() as u16
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let thresholds = DiscoverThresholds {
        min_composite_quality_q16: q16(knobs.discover_min_composite_quality),
        min_population_ratio_q16: q16(knobs.discover_min_population_ratio),
    };
    let quality = if hits.is_empty() {
        0.0
    } else {
        hits.iter().map(|h| f64::from(h.score)).sum::<f64>() / hits.len() as f64
    };
    let population = if k == 0 {
        1.0
    } else {
        (hits.len() as f64 / k as f64).min(1.0)
    };
    let (composite, ratio) = (q16(quality), q16(population));
    let mut failure_modes = Vec::new();
    if composite < thresholds.min_composite_quality_q16 {
        failure_modes.push(RetrievalFailureMode::LowRelevanceTopk);
    }
    if ratio < thresholds.min_population_ratio_q16
        || (hits.len() as u64) < knobs.retrieval_min_evidence
    {
        failure_modes.push(RetrievalFailureMode::SparseIndex);
    }
    let generation = if has_emb {
        core.semantic_store.read().generation()
    } else {
        0
    };
    RetrievalVerdict {
        gate_passed: failure_modes.is_empty(),
        above_threshold: composite >= thresholds.min_composite_quality_q16,
        failure_modes,
        composite_quality_q16: composite,
        index_population_ratio_q16: ratio,
        total_candidates: hits.len(),
        policy_revision: "discover-quality".to_string(),
        index_generation: generation,
        thresholds,
        adapter_digest: None,
        shard_generations: Vec::new(),
    }
}

/// One-round-trip hybrid discovery (CONCEPT:MEDULLA-KG.retrieval.one-round-trip-discovery).
///
/// Ranks nodes by BOTH lexical keyword overlap (over `name`/`description`/`type`)
/// AND semantic similarity to `query_embedding`, returning the top-`k` hydrated
/// with their human-readable text as `[{id,name,description,type,score}, …]`.
/// Complements [`Method::SemanticSearch`], which returns bare `(id, score)` and
/// leaves keyword matching + text hydration to the caller.
///
/// Candidate generation reuses the HNSW batch primitive
/// ([`SemanticStore::semantic_search`]) — over-fetched by [`DISCOVER_FANOUT`] so
/// the keyword re-rank has headroom — rather than an O(N) scan. Only when
/// `query_embedding` is empty (embedder/vLLM degraded) does it fall back to a
/// keyword-only scan, and even then bounded by [`DISCOVER_LEX_SCAN_CAP`].
///
/// Scoring (all reads are cheap in-memory, so this runs inline):
/// * both signals → `DISCOVER_SEM_WEIGHT · sim + DISCOVER_KW_WEIGHT · kw`;
/// * embedding only → `sim`;
/// * keywords only → `kw`.
fn discover(
    core: &GraphCore,
    keywords: &[String],
    query_embedding: &[f32],
    k: usize,
    hops: u32,
    req_id: u64,
) -> Response {
    // De-duplicate + lowercase the keyword set (order-independent).
    let kws = normalized_keywords(keywords);
    let has_kw = !kws.is_empty();
    let has_emb = !query_embedding.is_empty();

    // Per-candidate score components, keyed by node id.
    struct Hit {
        sim: f32,
        kw: f32,
    }
    let mut hits: std::collections::HashMap<String, Hit> = std::collections::HashMap::new();

    // 1. Dense candidates via the HNSW batch primitive (over-fetched for re-rank).
    if has_emb {
        let fanout = k.max(1).saturating_mul(DISCOVER_FANOUT).max(k);
        let raw = core
            .semantic_store
            .read()
            .semantic_search(query_embedding, fanout);
        for (id, sim) in raw {
            let kw = if has_kw {
                let (name, desc, ntype) = node_text(core, &id);
                keyword_overlap(&kws, &name, &desc, &ntype)
            } else {
                0.0
            };
            hits.insert(id, Hit { sim, kw });
        }
    } else if has_kw {
        // 2. Embedding-absent fallback: bounded keyword-only scan (documented
        //    degraded path — no HNSW batch primitive is usable without a vector).
        for id in core.node_ids().into_iter().take(DISCOVER_LEX_SCAN_CAP) {
            let (name, desc, ntype) = node_text(core, &id);
            let kw = keyword_overlap(&kws, &name, &desc, &ntype);
            if kw > 0.0 {
                hits.insert(id, Hit { sim: 0.0, kw });
            }
        }
    }

    // Combine, sort, take top-k, hydrate text.
    let mut ranked: Vec<(String, f32)> = hits
        .into_iter()
        .map(|(id, h)| {
            let score = if has_emb && has_kw {
                DISCOVER_SEM_WEIGHT * h.sim.clamp(0.0, 1.0) + DISCOVER_KW_WEIGHT * h.kw
            } else if has_emb {
                h.sim.clamp(0.0, 1.0)
            } else {
                h.kw
            };
            (id, score)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked.truncate(k);

    // MEDULLA-FEDERATED-QUERY-R090.8: neighbour expansion of the top-k hits.
    let planned = super::discover_plan::plan_discover(core, ranked, hops, k);

    let results: Vec<medulla_types::ingestion_wire::DiscoverHit> = planned
        .into_iter()
        .map(|hit| {
            let (name, description, node_type) = node_text(core, &hit.id);
            let web = web_provenance(core, &hit.id);
            medulla_types::ingestion_wire::DiscoverHit {
                url: web.url,
                retrieved_at: web.retrieved_at,
                backend: web.backend,
                query_digest: web.query_digest,
                hop_path: if hit.path.len() > 1 {
                    hit.path
                } else {
                    Vec::new()
                },
                id: hit.id,
                name,
                description,
                node_type,
                score: hit.score,
                arm_ranks: Vec::new(),
                recency_boost: 1.0,
            }
        })
        .collect();

    let verdict = discover_verdict(core, &results, k, has_emb);
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::Discover>(
            medulla_types::ingestion_wire::DiscoverResult {
                hits: results,
                verdict,
                plan: Default::default(),
            },
        ),
    )
}

/// `SemanticSearch`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_semantic_search(
    req_id: u64,
    core: &Arc<GraphCore>,
    query_embedding: Vec<f32>,
    n_results: usize,
) -> Response {
    // CONCEPT:MEDULLA-KG.txn.per-graph-write-isolation / Phase C-D — the HNSW index is now maintained
    // incrementally, so the ANN query is O(log n): hold the embedding read
    // lock only for the query itself (no whole-store clone, no per-query
    // index rebuild — at most a one-time lazy rebuild after load). The
    // per-candidate Ebbinghaus decay scoring still runs off-lock below.
    // Fetch more results initially to account for filtered-out nodes.
    let raw_results = {
        core.semantic_store
            .read()
            .semantic_search(&query_embedding, n_results * 2)
    };

    // Bounded candidate-metadata fetch: only the hit ids, not the graph.
    let candidates: Vec<(String, f32, Option<Vec<u8>>)> = {
        let g = &**core;
        raw_results
            .into_iter()
            .map(|(id, sim)| {
                let props = g.get_node_properties(&id);
                (id, sim, props)
            })
            .collect()
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match compute_off_lock(req_id, move || {
        weight_semantic_results(candidates, now, n_results)
    })
    .await
    {
        Ok(weighted_results) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::ingestion::SemanticSearch,
            >(&weighted_results),
        ),
        Err(resp) => resp,
    }
}

/// EH-396: re-aim a `SemanticSearch`/`Discover` query vector by the caller's
/// active adapter. `core` here is the caller's row-projected core, so the
/// adapted probe still searches only rows the caller may see.
#[cfg(feature = "decide")]
async fn adapt_query_vector(ctx: &GraphOpsContext<'_>, method: Method) -> Method {
    use medulla_server_ops::api::admin::served_adapter::adapted_query;
    use medulla_server_ops::api::admin::served_adapter::carrier_tenant;
    let tenant = carrier_tenant(Some(ctx.read_authority));
    let served = (ctx.graph_name, &**ctx.core);
    match method {
        Method::SemanticSearch {
            query_embedding,
            n_results,
        } => Method::SemanticSearch {
            query_embedding: adapted_query(ctx.state, tenant, served, query_embedding).await,
            n_results,
        },
        Method::Discover {
            keywords,
            query_embedding,
            k,
            quality_policy_ref,
            hops,
            query_text,
            mode,
            as_of,
        } => Method::Discover {
            keywords,
            query_embedding: adapted_query(ctx.state, tenant, served, query_embedding).await,
            k,
            quality_policy_ref,
            hops,
            query_text,
            mode,
            as_of,
        },
        other => other,
    }
}

#[cfg(not(feature = "decide"))]
async fn adapt_query_vector(_ctx: &GraphOpsContext<'_>, method: Method) -> Method {
    method
}

/// Handle semantic and embedding operations.
///
/// Every arm here is a READ, and every read is served from `core.semantic_store`
/// — the resident generation. That IS the live generation: the only thing that
/// makes an index resident and serving is
/// `SemanticStore::adopt_generation(&image)`, and the only image
/// `medulla_server::semantic_activation` ever adopts is the one
/// `SemanticCodeStore::read_live()` returns, which serves the live generation
/// and nothing else (a superseded or retired generation is unreachable through
/// it). So there is no second read path to add: taking a query through
/// `read_live()` would decode a whole IVF-PQ artifact per search and would bypass
/// the raw arena that `semantic_search`'s exact brute-force fallback needs while
/// an index is cold or warming.
///
/// `AddEmbedding` is the one mutating member of the family and it is
/// `mutation::GATEWAY_ROUTED`, so it is structurally unreachable here.
pub(super) async fn try_handle_semantic_compute(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext { req_id, core, .. } = ctx;
    let method = adapt_query_vector(&ctx, method).await;
    ControlFlow::Break(match method {
        Method::AddEmbedding { .. } => unreachable!(
            "AddEmbedding is mutation::GATEWAY_ROUTED; dispatch_graph_op must route it \
                 through try_handle_gateway before it ever reaches this terminal handler"
        ),
        Method::SemanticSearch {
            query_embedding,
            n_results,
        } => handle_semantic_search(req_id, core, query_embedding, n_results).await,
        Method::Discover {
            keywords,
            query_embedding,
            k,
            hops,
            ..
        } => discover(core, &keywords, &query_embedding, k, hops, req_id),
        // CONCEPT:MEDULLA-KG.compute.l2-normalize-batch-vectors — kernel-backed in-engine batch L2-normalize (compute-near-data).
        // The `numeric` feature links the pure medulla-numeric kernel (faer/ndarray, no Python-extension FFI);
        // a no-numeric build (e.g. `pi`) has no medulla-numeric, so the op reports it's absent.
        Method::BatchL2Normalize { vectors } => {
            #[cfg(feature = "numeric")]
            {
                let out = medulla_numeric::linalg::batch_l2_normalize(&vectors);
                Response::ok(
                    req_id,
                    ResultPayload::of::<
                        medulla_protocol_types::result_contract::compute::BatchL2Normalize,
                    >(out),
                )
            }
            #[cfg(not(feature = "numeric"))]
            {
                let _ = vectors;
                Response::err(
                    req_id,
                    "BatchL2Normalize requires the `numeric` feature (medulla-numeric kernel)."
                        .to_string(),
                )
            }
        }
        // AddEdge/RemoveEdge (CONCEPT:MEDULLA-P0-2 bypass guard): GATEWAY_ROUTED — see
        // the AddNode/RemoveNode comment above for why these are structurally
        // unreachable here, not merely undocumented.
        other => return ControlFlow::Continue(other),
    })
}
