//! `Method::ContextCompile` (MEDULLA-FEDERATED-QUERY-R650.3): compile one context
//! bundle for a query under a token budget.
//!
//! The handler runs the fixed front half of the pipeline over the caller's
//! row-projected core: validate the request, gather the lexical and semantic
//! arms, measure every hit at each resolution, drop what a read-time access
//! check refuses, then hand the ranked, measured hits to
//! `medulla_plan::context_bundle::compile`, which fuses, cuts and selects with
//! the one exact package solver. Nothing here writes. Rows the caller cannot
//! read are not in the projected core, so they never become candidates.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use medulla_decision_types::decision::context_bundle::{
    BundleKey, Citation, ContextBundle, ContextBundleRequest, ContextMode, DroppedReason,
    DroppedRecord, ExcludedItem, ExclusionReason, ProofEdge, MAX_TOP_N,
};
use medulla_exec::access_check::{
    filter_hits, AccessCache, AccessCheckAnswer, AccessCheckPort, AccessCheckRequest, AccessHit,
    AccessMode, DropReason, MAX_CACHE_TTL_MS,
};
use medulla_plan::context_bundle::{
    bundle_key_of, compile, CompileInputs, ContextKnobs, ResolutionSpan, SourceHit, SourceRanking,
};
use medulla_protocol_types::result_contract::query::ContextCompile;
use sha2::{Digest, Sha256};

/// Nodes the lexical arm scans, the bound `Discover` uses for its keyword scan.
const LEXICAL_SCAN_CAP: usize = 4096;
/// The semantic arm over-fetches so the cut has headroom.
const SEMANTIC_FANOUT: usize = 4;
/// Bytes of description the middle resolution keeps.
const SUMMARY_BYTES: usize = 240;
/// Source name of hits whose node names none.
const GRAPH_SOURCE: &str = "graph";
/// Edge relationship that supplies proof edges.
const DERIVED_FROM: &str = "DERIVED_FROM";

type Refusal = (&'static str, String);

fn refuse(code: &'static str, detail: impl Into<String>) -> Refusal {
    (code, detail.into())
}

fn refusal_response(req_id: u64, (code, detail): Refusal) -> Response {
    Response::err(req_id, format!("{code}: {detail}"))
}

/// A source that decides access only at read time and has no registered port:
/// every check is unavailable, so its hits fail closed (R703.4).
struct NoAccessPort;

impl AccessCheckPort for NoAccessPort {
    fn check(&mut self, _request: &AccessCheckRequest) -> Option<(AccessCheckAnswer, u64)> {
        None
    }
}

/// What the request must carry before any row is read.
fn validate(request: &ContextBundleRequest) -> Result<(), Refusal> {
    let budget = &request.budget;
    if budget.window_tokens == 0
        || request.tokenizer_id.trim().is_empty()
        || request.tokenizer_version.trim().is_empty()
    {
        return Err(refuse(
            "CONTEXT_BUDGET_MISSING",
            "a bundle needs a model window and the tokenizer that measures it",
        ));
    }
    if !request.top_n_in_range() {
        return Err(refuse(
            "INVALID_ARGUMENT",
            format!("top_n must be in [1, {MAX_TOP_N}]"),
        ));
    }
    if request.query_embedding.iter().any(|v| !v.is_finite()) {
        return Err(refuse(
            "CONTEXT_ITEM_COST_INVALID",
            "the query embedding holds a non-finite value",
        ));
    }
    if request.mode == ContextMode::Semantic && request.query_embedding.is_empty() {
        return Err(refuse(
            "INVALID_ARGUMENT",
            "semantic mode needs a query embedding",
        ));
    }
    if request.mode != ContextMode::Semantic && request.query_text.trim().is_empty() {
        return Err(refuse(
            "INVALID_ARGUMENT",
            "lexical and hybrid modes need query text",
        ));
    }
    Ok(())
}

fn node_object(core: &GraphCore, id: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.get_node_properties(id)?;
    Some(medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default())
}

fn text_field(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// What the arms and the measurement read of one node.
struct NodeText {
    name: String,
    description: String,
    node_type: String,
    source: String,
    mode: AccessMode,
}

fn node_text(core: &GraphCore, id: &str) -> Option<NodeText> {
    let object = node_object(core, id)?;
    let name = text_field(&object, "name");
    let node_type = match text_field(&object, "type") {
        t if t.is_empty() => text_field(&object, "node_type"),
        t => t,
    };
    let source = text_field(&object, "source");
    Some(NodeText {
        name: if name.is_empty() {
            id.to_string()
        } else {
            name
        },
        description: text_field(&object, "description"),
        node_type,
        source: if source.is_empty() {
            GRAPH_SOURCE.to_string()
        } else {
            source
        },
        mode: if text_field(&object, "access_mode") == "checked_at_read" {
            AccessMode::CheckedAtRead
        } else {
            AccessMode::Synced
        },
    })
}

/// The distinct lowercase words of the query.
fn query_words(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

/// Fraction of the query words found in the node's text.
fn keyword_overlap(words: &[String], node: &NodeText) -> f32 {
    if words.is_empty() {
        return 0.0;
    }
    let haystack = format!(
        "{} {} {}",
        node.name.to_lowercase(),
        node.description.to_lowercase(),
        node.node_type.to_lowercase()
    );
    let matched = words
        .iter()
        .filter(|w| haystack.contains(w.as_str()))
        .count();
    matched as f32 / words.len() as f32
}

fn truncate_at_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn digest_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Measure a node at its distinct resolutions: the name, the name with a short
/// description, and the name with the whole description.
fn spans_of(id: &str, node: &NodeText, revision: u64, bytes_per_token: u64) -> Vec<ResolutionSpan> {
    let mut texts = vec![node.name.clone()];
    if !node.description.is_empty() {
        let short = truncate_at_boundary(&node.description, SUMMARY_BYTES);
        texts.push(format!("{}\n{}", node.name, short));
        if short.len() < node.description.len() {
            texts.push(format!("{}\n{}", node.name, node.description));
        }
    }
    texts
        .into_iter()
        .enumerate()
        .map(|(resolution, text)| ResolutionSpan {
            resolution: resolution as u8,
            tokens: (text.len() as u64).div_ceil(bytes_per_token.max(1)).max(1),
            citation: Citation {
                source_id: id.to_string(),
                cited_start: 0,
                cited_end: text.len() as u64,
                source_revision: revision,
                content_digest: digest_hex(&text),
            },
        })
        .collect()
}

/// `DERIVED_FROM` edges that leave each hit, the proof edges of its item.
fn derived_edges(core: &GraphCore, ids: &[String]) -> BTreeMap<String, Vec<ProofEdge>> {
    let mut out: BTreeMap<String, Vec<ProofEdge>> = BTreeMap::new();
    for (from, neighbours) in core.get_neighbors_batch(ids.to_vec()) {
        for to in neighbours {
            for blob in core.get_edge_properties(&from, &to) {
                let object =
                    medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default();
                let relationship = ["relationship", "type", "edge_type"]
                    .iter()
                    .map(|key| text_field(&object, key))
                    .find(|v| !v.is_empty())
                    .unwrap_or_default();
                if relationship == DERIVED_FROM {
                    out.entry(from.clone()).or_default().push(ProofEdge {
                        from: from.clone(),
                        relationship,
                        to: to.clone(),
                    });
                }
            }
        }
    }
    for edges in out.values_mut() {
        edges.sort_by(|a, b| a.to.cmp(&b.to));
        edges.dedup();
    }
    out
}

fn finite_score(score: f32) -> Result<f32, Refusal> {
    if score.is_finite() {
        Ok(score)
    } else {
        Err(refuse(
            "CONTEXT_ITEM_COST_INVALID",
            "an arm produced a non-finite item score",
        ))
    }
}

/// Score ids by the lexical and semantic arms; best first, at most `top_n`.
fn arm_scores(
    core: &GraphCore,
    request: &ContextBundleRequest,
) -> Result<Vec<(&'static str, Vec<(String, f32)>)>, Refusal> {
    let top_n = request.top_n as usize;
    let mut arms = Vec::new();
    if request.mode != ContextMode::Semantic {
        let words = query_words(&request.query_text);
        let mut scored = Vec::new();
        for id in core.node_ids().into_iter().take(LEXICAL_SCAN_CAP) {
            if let Some(node) = node_text(core, &id) {
                let score = finite_score(keyword_overlap(&words, &node))?;
                if score > 0.0 {
                    scored.push((id, score));
                }
            }
        }
        arms.push(("lexical", scored));
    }
    if request.mode != ContextMode::Lexical && !request.query_embedding.is_empty() {
        let raw = core.semantic_store.read().semantic_search(
            &request.query_embedding,
            top_n.saturating_mul(SEMANTIC_FANOUT),
        );
        let mut scored = Vec::new();
        for (id, similarity) in raw {
            scored.push((id, finite_score(similarity)?.clamp(0.0, 1.0)));
        }
        arms.push(("semantic", scored));
    }
    for (_, scored) in &mut arms {
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(top_n);
    }
    Ok(arms)
}

/// The compile result and whether any hit needed a read-time check (such a
/// bundle is never cached: its answer is only as fresh as the check).
struct Compiled {
    bundle: ContextBundle,
    cacheable: bool,
}

fn run_compile(
    core: &GraphCore,
    request: &ContextBundleRequest,
    tenant: &str,
    principal: &str,
    policy_digest: &str,
) -> Result<Compiled, Refusal> {
    let knobs = medulla_server_api::knob_config::knobs();
    let bytes_per_token = knobs.context_facts_bytes_per_token;
    let revision = core.version();
    let arms = arm_scores(core, request)?;

    // One read per distinct hit.
    let ids: BTreeSet<String> = arms
        .iter()
        .flat_map(|(_, scored)| scored.iter().map(|(id, _)| id.clone()))
        .collect();
    let mut nodes: BTreeMap<String, Option<NodeText>> = BTreeMap::new();
    for id in &ids {
        nodes.insert(id.clone(), node_text(core, id));
    }

    // Read-time access check before anything is fused or cut.
    let hits: Vec<AccessHit> = nodes
        .iter()
        .filter_map(|(id, node)| {
            node.as_ref().map(|n| AccessHit {
                source: n.source.clone(),
                record_ref: id.clone(),
                mode: n.mode,
            })
        })
        .collect();
    let cacheable = hits.iter().all(|h| h.mode == AccessMode::Synced);
    let mut cache = AccessCache::new(MAX_CACHE_TTL_MS);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let filtered = filter_hits(hits, principal, &mut NoAccessPort, &mut cache, now_ms);
    let mut pre_excluded = Vec::new();
    let mut unchecked_dropped = Vec::new();
    let mut gone: BTreeSet<String> = BTreeSet::new();
    for (hit, reason) in &filtered.dropped {
        gone.insert(hit.record_ref.clone());
        pre_excluded.push(ExcludedItem {
            item_id: hit.record_ref.clone(),
            reason: ExclusionReason::Denied,
        });
        unchecked_dropped.push(DroppedRecord {
            record_id: hit.record_ref.clone(),
            reason: match reason {
                DropReason::Denied => DroppedReason::Denied,
                DropReason::Timeout => DroppedReason::Timeout,
                DropReason::Unavailable => DroppedReason::Unavailable,
            },
        });
    }

    let kept: Vec<String> = ids
        .iter()
        .filter(|id| !gone.contains(*id))
        .cloned()
        .collect();
    let proof = derived_edges(core, &kept);
    let rankings = arms
        .into_iter()
        .map(|(source, scored)| SourceRanking {
            source: source.to_string(),
            curve: None,
            hits: scored
                .into_iter()
                .filter(|(id, _)| !gone.contains(id))
                .map(|(id, score)| {
                    let spans = match nodes.get(&id) {
                        Some(Some(node)) => spans_of(&id, node, revision, bytes_per_token),
                        _ => Vec::new(),
                    };
                    SourceHit {
                        derived_from: proof.get(&id).cloned().unwrap_or_default(),
                        node_id: id,
                        score,
                        spans,
                        hop_path: Vec::new(),
                    }
                })
                .collect(),
        })
        .collect();

    let inputs = CompileInputs {
        tenant: tenant.to_string(),
        graph_revision: revision,
        policy_digest: policy_digest.to_string(),
        rankings,
        pre_excluded,
        unchecked_dropped,
        knobs: ContextKnobs {
            rrf_k: u32::try_from(knobs.context_rrf_k).unwrap_or(60),
            min_items: u32::try_from(knobs.context_min_items).unwrap_or(3),
            solver_node_budget: u32::try_from(knobs.context_solver_node_budget).unwrap_or(100_000),
            proof_edges_per_item: u32::try_from(knobs.context_proof_edges_per_item).unwrap_or(3),
            value_floor_micro_per_token: knobs.context_value_floor_micro_per_token,
        },
    };
    let bundle = compile(request, &inputs).map_err(|refusal| {
        let code = refusal.code.as_str();
        // The package codes are registered strings; the router needs a static.
        let code: &'static str = match code {
            "CONTEXT_PACKAGE_MISSING_FACT" => "CONTEXT_PACKAGE_MISSING_FACT",
            _ => "CONTEXT_PACKAGE_INVALID_MEASUREMENT",
        };
        refuse(code, refusal.detail)
    })?;
    Ok(Compiled { bundle, cacheable })
}

/// The caller-bound part of the cache key, so one caller's bundle never serves
/// another caller with different row visibility.
fn policy_digest(ctx: &QueryHandlerCtx<'_>) -> String {
    format!(
        "{:032x}",
        rls_cache_hash(
            "context_compile_policy",
            b"",
            #[cfg(feature = "security")]
            ctx.caller,
            #[cfg(feature = "security")]
            ctx.rls,
        )
    )
}

/// Serve one `ContextCompile`.
pub(crate) async fn handle_context_compile(
    ctx: &QueryHandlerCtx<'_>,
    request: ContextBundleRequest,
) -> Response {
    let req_id = ctx.req_id;
    if let Err(refusal) = validate(&request) {
        return refusal_response(req_id, refusal);
    }
    // An agent reference must name an agent the caller's tenant owns and can
    // see; the projected core holds no other tenant's rows.
    if let Some(agent) = &request.agent_ref {
        if ctx.core.get_node_properties(agent).is_none() {
            return refusal_response(
                req_id,
                refuse(
                    "CONTEXT_SCOPE_FOREIGN",
                    format!("agent {agent} is not in this tenant"),
                ),
            );
        }
    }
    let revision = ctx.core.version();
    if let Some(as_of) = request.as_of {
        if as_of != revision {
            return refusal_response(
                req_id,
                refuse(
                    "INVALID_ARGUMENT",
                    format!("as_of {as_of} is not the served revision {revision}"),
                ),
            );
        }
    }
    let tenant = ctx.graph_name.to_string();
    let policy = policy_digest(ctx);
    let key: BundleKey = bundle_key_of(&request, &tenant, revision, &policy);
    #[cfg(feature = "result-cache")]
    let cache_key = Some((
        rls_cache_hash(
            "context_compile",
            key.0.as_bytes(),
            #[cfg(feature = "security")]
            ctx.caller,
            #[cfg(feature = "security")]
            ctx.rls,
        ),
        None,
    ));
    #[cfg(feature = "result-cache")]
    if let Some(response) =
        cached_served_response::<ContextCompile>(req_id, ctx.core, ctx.graph_name, &cache_key)
    {
        return response;
    }
    #[cfg(feature = "result-cache")]
    let started = std::time::Instant::now();

    let core = Arc::clone(ctx.core);
    let principal = ctx.caller.to_string();
    let worker_tenant = tenant.clone();
    let worker_request = request;
    let outcome = compute_off_lock(req_id, move || {
        run_compile(&core, &worker_request, &worker_tenant, &principal, &policy)
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok(Err(refusal)) => refusal_response(req_id, refusal),
        Ok(Ok(compiled)) => {
            debug_assert_eq!(compiled.bundle.key, key);
            served_response::<ContextCompile>(
                req_id,
                Ok(Ok(compiled.bundle)),
                "ContextCompile",
                #[cfg(feature = "result-cache")]
                ctx.core,
                #[cfg(feature = "result-cache")]
                cache_key
                    .filter(|_| compiled.cacheable)
                    .map(|(hash, dep)| CacheSlot {
                        hash,
                        dep,
                        version: revision,
                        tenant,
                        started,
                    }),
            )
        }
    }
}
