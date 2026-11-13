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

/// A node's timestamp in unix seconds: `updated_at`, else `created_at`
/// (MEDULLA-FEDERATED-QUERY-R090.2). Absent or non-numeric is `None`.
fn node_time(core: &GraphCore, node_id: &str) -> Option<i64> {
    let blob = core.get_node_properties(node_id)?;
    let obj = super::gateway_graph::decode_json_object(&blob);
    ["updated_at", "created_at"].iter().find_map(|key| {
        let value = obj.get(*key)?;
        value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
    })
}

/// The half life of a node type: the `recencyHalfLifeDays` annotation on the type's class
/// node (the node whose id is the type name), else `knob discover.recency_half_life_days`.
fn half_life_days(core: &GraphCore, node_type: &str, default_days: f64) -> f64 {
    core.get_node_properties(node_type)
        .and_then(|blob| {
            super::gateway_graph::decode_json_object(&blob)
                .get("recencyHalfLifeDays")
                .and_then(serde_json::Value::as_f64)
        })
        .filter(|days| *days > 0.0)
        .unwrap_or(default_days)
}

/// The candidates of one `Discover` call: every node the dense or keyword path reached,
/// with its two raw signals.
struct Candidate {
    id: String,
    sim: f32,
    kw: f32,
}

struct Candidates {
    list: Vec<Candidate>,
    /// The keyword-only scan stopped at [`DISCOVER_LEX_SCAN_CAP`].
    scan_capped: bool,
}

/// Dense candidates (over-fetched for the keyword re-rank) or, with no embedding, the
/// bounded keyword-only scan.
fn gather_candidates(
    core: &GraphCore,
    kws: &[String],
    query_embedding: &[f32],
    k: usize,
) -> Candidates {
    let has_kw = !kws.is_empty();
    let mut list = Vec::new();
    let mut scan_capped = false;
    if !query_embedding.is_empty() {
        let fanout = k.max(1).saturating_mul(DISCOVER_FANOUT).max(k);
        let raw = core
            .semantic_store
            .read()
            .semantic_search(query_embedding, fanout);
        for (id, sim) in raw {
            let kw = if has_kw {
                let (name, desc, ntype) = node_text(core, &id);
                keyword_overlap(kws, &name, &desc, &ntype)
            } else {
                0.0
            };
            list.push(Candidate { id, sim, kw });
        }
    } else if has_kw {
        let ids = core.node_ids();
        scan_capped = ids.len() > DISCOVER_LEX_SCAN_CAP;
        for id in ids.into_iter().take(DISCOVER_LEX_SCAN_CAP) {
            let (name, desc, ntype) = node_text(core, &id);
            let kw = keyword_overlap(kws, &name, &desc, &ntype);
            if kw > 0.0 {
                list.push(Candidate { id, sim: 0.0, kw });
            }
        }
    }
    Candidates { list, scan_capped }
}

/// One-round-trip hybrid discovery (CONCEPT:MEDULLA-KG.retrieval.one-round-trip-discovery),
/// planned by [`discover_plan::plan_discover`] (MEDULLA-FEDERATED-QUERY-R090.2).
///
/// Mode `standard` ranks by the weighted blend: both signals give
/// `DISCOVER_SEM_WEIGHT * sim + DISCOVER_KW_WEIGHT * kw`, one signal gives that signal.
/// Mode `deep` fuses the semantic arm, the keyword arm and one semantic arm per
/// expansion by reciprocal rank. A recency boost applies in `deep` mode or when `as_of`
/// is set. Hits then expand over `hops` and hydrate. The verdict is
/// [`discover_quality::quality_verdict`] and the plan carries the expansion evidence.
#[cfg(feature = "query")]
struct DiscoverCall<'a> {
    keywords: &'a [String],
    query_embedding: &'a [f32],
    k: usize,
    hops: u32,
    as_of: Option<i64>,
    graph_name: &'a str,
}

#[cfg(feature = "query")]
fn discover(
    core: &GraphCore,
    call: &DiscoverCall<'_>,
    deep: super::discover_plan::DeepArms,
    req_id: u64,
) -> Response {
    use super::discover_plan::{fuse_arms, recency_boost, ArmList};
    use medulla_types::ingestion_wire::{DiscoverArm, DiscoverPlan};

    let k = call.k;
    let knobs = medulla_server_api::knob_config::knobs();
    let kws = normalized_keywords(call.keywords);
    let has_emb = !call.query_embedding.is_empty();
    let has_kw = !kws.is_empty();
    let gathered = gather_candidates(core, &kws, call.query_embedding, k);
    let is_deep = !deep.vectors.is_empty();

    // Score: the weighted blend (standard) or reciprocal-rank fusion over the arms (deep).
    let mut arm_ranks: std::collections::HashMap<
        String,
        Vec<medulla_types::ingestion_wire::ArmRank>,
    > = std::collections::HashMap::new();
    let mut ranked: Vec<(String, f32)> = if is_deep {
        let by_score = |mut v: Vec<(String, f32)>| {
            v.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
            v
        };
        let mut arms = Vec::new();
        if has_emb {
            arms.push(ArmList {
                arm: DiscoverArm::Semantic,
                ranked: by_score(
                    gathered
                        .list
                        .iter()
                        .map(|c| (c.id.clone(), c.sim))
                        .collect(),
                ),
            });
        }
        if has_kw {
            arms.push(ArmList {
                arm: DiscoverArm::Keyword,
                ranked: by_score(
                    gathered
                        .list
                        .iter()
                        .filter(|c| c.kw > 0.0)
                        .map(|c| (c.id.clone(), c.kw))
                        .collect(),
                ),
            });
        }
        let fanout = k.max(1).saturating_mul(DISCOVER_FANOUT).max(k);
        for vector in &deep.vectors {
            let raw = core.semantic_store.read().semantic_search(vector, fanout);
            arms.push(ArmList {
                arm: DiscoverArm::Expansion,
                ranked: raw,
            });
        }
        fuse_arms(&arms, knobs.discover_rrf_k)
            .into_iter()
            .map(|fused| {
                arm_ranks.insert(fused.id.clone(), fused.arm_ranks);
                (fused.id, fused.score)
            })
            .collect()
    } else {
        gathered
            .list
            .iter()
            .map(|c| {
                let score = if has_emb && has_kw {
                    DISCOVER_SEM_WEIGHT * c.sim.clamp(0.0, 1.0) + DISCOVER_KW_WEIGHT * c.kw
                } else if has_emb {
                    c.sim.clamp(0.0, 1.0)
                } else {
                    c.kw
                };
                (c.id.clone(), score)
            })
            .collect()
    };

    // Recency: deep mode, or any request that names `as_of`.
    let mut boosts: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    if is_deep || call.as_of.is_some() {
        let as_of = call.as_of.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        });
        for (id, score) in &mut ranked {
            let (_, _, node_type) = node_text(core, id);
            let half_life = half_life_days(
                core,
                &node_type,
                knobs.discover_recency_half_life_days as f64,
            );
            let boost = recency_boost(
                knobs.discover_recency_weight,
                half_life,
                as_of,
                node_time(core, id),
            );
            *score *= boost;
            boosts.insert(id.clone(), boost);
        }
    }
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked.truncate(k);

    // MEDULLA-FEDERATED-QUERY-R090.8: neighbour expansion of the top-k hits.
    let planned = super::discover_plan::expand_seeds(core, ranked, call.hops, k);

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
                arm_ranks: arm_ranks.remove(&hit.id).unwrap_or_default(),
                recency_boost: boosts.get(&hit.id).copied().unwrap_or(1.0),
                id: hit.id,
                name,
                description,
                node_type,
                score: hit.score,
            }
        })
        .collect();

    let (embedded_nodes, index_generation, embedded_through) = {
        let store = core.semantic_store.read();
        (
            store.len(),
            store.generation(),
            store.embedded_through_version(),
        )
    };
    let signals = medulla_retrieval::discover_signals::discover_signals()
        .map(|source| source.read(call.graph_name))
        .unwrap_or_default();
    let verdict = super::discover_quality::quality_verdict(
        &super::discover_quality::QualityInputs {
            hits: &results,
            has_embedding: has_emb,
            keyword_scan_capped: gathered.scan_capped,
            embedded_nodes,
            candidate_nodes: core.node_count(),
            graph_version: core.version(),
            embedded_through,
            index_generation,
            signals,
        },
        &super::discover_quality::QualityKnobs::from_registry(),
    );
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::Discover>(
            medulla_types::ingestion_wire::DiscoverResult {
                hits: results,
                verdict,
                plan: DiscoverPlan {
                    expansions: deep.evidence,
                    degraded: deep.degraded,
                },
            },
        ),
    )
}

/// Serve `Discover`: plan off the reactor (the provider and embedder are blocking network
/// clients; one provider call per request), then rank. A planner refusal returns its wire
/// code, no rows and writes nothing.
#[cfg(feature = "query")]
#[allow(clippy::too_many_arguments)]
async fn handle_discover(
    req_id: u64,
    core: &Arc<GraphCore>,
    graph_name: &str,
    keywords: Vec<String>,
    query_embedding: Vec<f32>,
    k: usize,
    hops: u32,
    query_text: Option<String>,
    mode: medulla_types::ingestion_wire::DiscoverMode,
    as_of: Option<i64>,
) -> Response {
    let request = medulla_types::ingestion_wire::DiscoverPlanRequest {
        query_text,
        mode,
        as_of,
        hops,
    };
    let expansions = medulla_server_api::knob_config::knobs().discover_expansions as usize;
    let planned = match compute_off_lock(req_id, move || {
        super::discover_plan::plan_discover(&request, expansions)
    })
    .await
    {
        Ok(planned) => planned,
        Err(response) => return response,
    };
    let deep = match planned {
        Ok(deep) => deep,
        Err(refusal) => return Response::err(req_id, refusal.message()),
    };
    discover(
        core,
        &DiscoverCall {
            keywords: &keywords,
            query_embedding: &query_embedding,
            k,
            hops,
            as_of,
            graph_name,
        },
        deep,
        req_id,
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
        #[cfg(feature = "query")]
        Method::Discover {
            keywords,
            query_embedding,
            k,
            hops,
            query_text,
            mode,
            as_of,
            ..
        } => {
            handle_discover(
                req_id,
                core,
                ctx.graph_name,
                keywords,
                query_embedding,
                k,
                hops,
                query_text,
                mode,
                as_of,
            )
            .await
        }
        #[cfg(not(feature = "query"))]
        Method::Discover { .. } => Response::err(
            req_id,
            "Discover requires the `query` feature (the retrieval stop rule)".to_string(),
        ),
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
