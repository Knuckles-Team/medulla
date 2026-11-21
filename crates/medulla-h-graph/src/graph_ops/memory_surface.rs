//! The served memory surface (MEDULLA-DECISION-ENGINE-R651.3).
//!
//! `MemoryRemember` is gateway-routed (`Route::MemorySurface`): the record is
//! admitted against the stored node, then commits as the one `AddNode` upsert
//! through the staged gateway, so it is durable and replicated like every other
//! graph write. `MemoryRecall` and `MemoryExport` are reads over the caller's
//! projected graph view. Every record is stored and read under the verified
//! request tenant; another tenant's record reads as absent.
//!
//! A record is one node of its tier's node type (`WorkingMemory`,
//! `EpisodicMemory`, `ProceduralMemory`, `SemanticMemory`). Its properties are
//! `MemoryRecord::to_props` plus the tenant scope, so the `Reinforce` and
//! `DecayMemories` routes keep maintaining the same `importance`,
//! `access_count`, `last_access_ms`, `last_decay_ms` and `forgotten`
//! properties. Its embedding is written to the graph's vector index by the
//! existing `AddEmbedding` commit, right after the node. Recall finds
//! candidates by cosine on that index, restricted to the tenant's memory
//! nodes, and blends the cosine with keyword overlap over the text, the topics
//! and the derivation text of each attachment. The consolidation plan takes
//! each record's neighbours from the same index. Ranking, budget and plan are
//! deterministic.

use std::collections::{BTreeMap, BTreeSet};

use super::terminal::GraphOpsContext;
use super::*;
use medulla_decision_stats::decision::memory as kernel;
use medulla_decision_types::decision::knobs::{KnobSpec, KnobValue};
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::graph::{
    MemoryExport as MemoryExportResultKind, MemoryRecall as MemoryRecallResultKind,
    MemoryRemember as MemoryRememberResultKind,
};
use medulla_modality::media_memory::match_derivation;
use medulla_types::contract::ResourceId;
use medulla_work_types::memory::{
    MemoryExportManifest, MemoryExportRequest, MemoryExportResult, MemoryHit, MemoryRecallRequest,
    MemoryRecallResult, MemoryRecord, MemoryRememberRequest, MemoryRememberResult, MemoryTier,
    MEMORY_RECORD_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

const TENANT_KEY: &str = "tenant_scope";
const DERIVATION_TEXT_KEY: &str = "derivationText";

/// The tiers in export and scan order.
const TIERS: [MemoryTier; 4] = [
    MemoryTier::Working,
    MemoryTier::Episodic,
    MemoryTier::Procedural,
    MemoryTier::Semantic,
];
/// The tiers `MemoryExport` returns.
const EXPORT_TIERS: [MemoryTier; 2] = [MemoryTier::Procedural, MemoryTier::Semantic];
/// Most nodes one tier scan reads.
const SCAN_CAP_PER_TIER: usize = 50_000;
/// Most records one consolidation plan covers.
const PLAN_RECORD_CAP: usize = 2_048;
/// Most neighbours kept per record for the plan.
const NEIGHBOURS_PER_RECORD: usize = 8;
/// Most candidates one embedding search returns from the vector index.
const SEARCH_FANOUT: usize = 256;
/// Blend weights when both an embedding and keywords are given.
const SEMANTIC_WEIGHT: f64 = 0.7;
const KEYWORD_WEIGHT: f64 = 0.3;
const Q16_ONE: f64 = 65_536.0;

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn knob_decimal(spec: &KnobSpec) -> f64 {
    match spec.default {
        KnobValue::DecimalMicros(micros) => micros as f64 / 1_000_000.0,
        KnobValue::Integer(value) => value as f64,
        KnobValue::DurationMs(value) => value as f64,
    }
}

fn knob_integer(spec: &KnobSpec) -> i64 {
    match (spec.default, spec.min, spec.max) {
        (KnobValue::Integer(value), _, _) => value,
        _ => 0,
    }
}

fn knob_bounds(spec: &KnobSpec) -> (i64, i64) {
    match (spec.min, spec.max) {
        (KnobValue::Integer(min), KnobValue::Integer(max)) => (min, max),
        _ => (0, 0),
    }
}

/// The node id of one record of one tenant.
fn node_id(tenant: &str, memory_id: &str) -> String {
    format!("memory:{tenant}:{memory_id}")
}

/// One stored record.
struct Stored {
    record: MemoryRecord,
}

/// The id prefix of every memory node of `tenant`.
fn tenant_prefix(tenant: &str) -> String {
    format!("memory:{tenant}:")
}

/// Decode one node of `tenant`; anything else reads as absent.
fn decode(blob: &[u8], tenant: &str) -> Option<Stored> {
    let mut props = medulla_types::msgpack::decode_property_object(blob).ok()?;
    if props.remove(TENANT_KEY)?.as_str()? != tenant {
        return None;
    }
    let record = MemoryRecord::from_props(&props).ok()?;
    (record.tenant.as_str() == tenant).then_some(Stored { record })
}

fn read_stored(core: &GraphCore, tenant: &str, memory_id: &str) -> Option<Stored> {
    decode(&core.get_node_properties(&node_id(tenant, memory_id))?, tenant)
}

/// Every record of `tenant` in the named tiers, in `memory_id` order per tier.
fn load(core: &GraphCore, tenant: &str, tiers: &[MemoryTier]) -> Vec<Stored> {
    let mut stored: Vec<Stored> = tiers
        .iter()
        .flat_map(|tier| core.get_nodes_by_label(tier.node_type(), SCAN_CAP_PER_TIER))
        .filter_map(|(_, blob)| decode(&blob, tenant))
        .collect();
    stored.sort_by(|a, b| {
        a.record
            .tier
            .cmp(&b.record.tier)
            .then_with(|| a.record.memory_id.as_str().cmp(b.record.memory_id.as_str()))
    });
    stored
}

fn node_blob(tenant: &str, request: &MemoryRememberRequest) -> Result<Vec<u8>, String> {
    let mut props = request.record.to_props();
    props.insert(TENANT_KEY.into(), tenant.into());
    rmp_serde::to_vec_named(&serde_json::Value::Object(props)).map_err(|error| {
        refuse("PARAMETER_INVALID", format!("record encoding failed: {error}"))
    })
}

/// Two records that differ only in the fields the maintenance routes move are
/// the same record.
fn same_identity(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    let strip = |record: &MemoryRecord| {
        let mut record = record.clone();
        record.importance = 0.0;
        record.access_count = 0;
        record.last_access_ms = 0;
        record.last_decay_ms = 0;
        record.forgotten = false;
        record
    };
    strip(a) == strip(b)
}

/// Whether the indexed vector is the one a repeat remember carries. The index
/// may store a normalized copy, so agreement is cosine, not equality.
fn vectors_agree(stored: &[f32], given: &[f32]) -> bool {
    if stored.len() != given.len() || stored.is_empty() {
        return false;
    }
    let dot: f64 = stored.iter().zip(given).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    let (a, b) = (norm(stored), norm(given));
    a > 0.0 && b > 0.0 && dot / (a * b) >= 0.999_999
}

fn check_embedding(embedding: &[f32], what: &str) -> Result<(), String> {
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(refuse(
            "PARAMETER_INVALID",
            format!("{what} holds a value that is not finite"),
        ));
    }
    Ok(())
}

/// Whether `digest` is 64 lowercase hex characters.
fn is_sha256_hex(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What an admitted remember does.
pub(super) enum Admitted {
    /// The node upsert to commit, then the vector write beside it, and the answer.
    Write(Box<Method>, Option<Box<Method>>, MemoryRememberResult),
    /// The node is already stored but its vector is not indexed: write the vector only.
    Embed(Box<Method>, MemoryRememberResult),
    /// The very same record and vector are already stored: nothing to commit.
    Same(MemoryRememberResult),
}

/// Admit one remember against the stored node and the vector index, writing nothing.
///
/// `known_digests` holds the request's attachment digests the caller's owner scope holds in the
/// blob ledger (MEDULLA-MEDIA-VOCAB-R650.3); an attachment outside it refuses `MEDIA_BLOB_UNKNOWN`
/// before any write.
pub(super) fn admit_remember(
    core: &GraphCore,
    tenant: &str,
    request: &MemoryRememberRequest,
    known_digests: &BTreeSet<String>,
) -> Result<Admitted, String> {
    let record = &request.record;
    if record.tenant.as_str() != tenant {
        return Err(refuse(
            "ACCESS_DENIED",
            "a memory record may only be stored under the verified tenant",
        ));
    }
    if record.schema_version != MEMORY_RECORD_SCHEMA_VERSION {
        return Err(refuse(
            "PARAMETER_INVALID",
            format!("memory record schema version {} is not served", record.schema_version),
        ));
    }
    if record.text.is_empty() || !record.importance.is_finite() {
        return Err(refuse(
            "PARAMETER_INVALID",
            "a memory record needs text and a finite importance",
        ));
    }
    check_embedding(&request.embedding, "the embedding")?;
    if let Some(attachment) = record
        .attachments
        .iter()
        .find(|attachment| !is_sha256_hex(&attachment.blob_digest))
    {
        return Err(refuse(
            "PARAMETER_INVALID",
            format!(
                "attachment digest '{}' is not 64 lowercase hex characters",
                attachment.blob_digest
            ),
        ));
    }
    if let Some(attachment) = record
        .attachments
        .iter()
        .find(|attachment| !known_digests.contains(&attachment.blob_digest))
    {
        return Err(refuse(
            "MEDIA_BLOB_UNKNOWN",
            format!(
                "attachment digest '{}' is not held by the caller's owner scope",
                attachment.blob_digest
            ),
        ));
    }
    let id = node_id(tenant, record.memory_id.as_str());
    let answer = |stored| MemoryRememberResult {
        memory_id: record.memory_id.clone(),
        stored,
    };
    let embed = |embedding: &[f32]| {
        (!embedding.is_empty()).then(|| {
            Box::new(Method::AddEmbedding {
                node_id: id.clone(),
                embedding: embedding.to_vec(),
            })
        })
    };
    let Some(existing) = read_stored(core, tenant, record.memory_id.as_str()) else {
        let write = Method::AddNode {
            node_id: id.clone(),
            properties_msgpack: node_blob(tenant, request)?,
        };
        return Ok(Admitted::Write(Box::new(write), embed(&request.embedding), answer(true)));
    };
    let indexed = core.semantic_store.read().get_embedding(&id);
    let conflict = || {
        refuse(
            "PARAMETER_INVALID",
            format!(
                "memory record {} is already stored with a different body",
                record.memory_id.as_str()
            ),
        )
    };
    if !same_identity(&existing.record, record) {
        return Err(conflict());
    }
    match (indexed, request.embedding.is_empty()) {
        (_, true) => Ok(Admitted::Same(answer(false))),
        (Some(stored), false) if vectors_agree(&stored, &request.embedding) => Ok(Admitted::Same(answer(false))),
        (Some(_), false) => Err(conflict()),
        (None, false) => Ok(Admitted::Embed(
            embed(&request.embedding).expect("a non-empty embedding"),
            answer(true),
        )),
    }
}

/// The attachment digests of a `MemoryRemember` that the caller's owner scope holds in the blob
/// ledger (MEDULLA-MEDIA-VOCAB-R650.3). Empty for any other method, for a build without the
/// `blob` feature and for a request with no verified carrier.
pub(super) async fn known_attachment_digests(
    state: &Arc<RwLock<ServerState>>,
    owner_scope: &str,
    method: &Method,
) -> BTreeSet<String> {
    let Method::MemoryRemember { request } = method else {
        return BTreeSet::new();
    };
    if request.record.attachments.is_empty() {
        return BTreeSet::new();
    }
    #[cfg(feature = "blob")]
    {
        use medulla_server_blob::blob::store::ChunkStore;
        use medulla_server_blob::blob::BlobStateExt;
        let Ok(holder) = medulla_server_blob::blob::store::HolderId::for_owner(owner_scope) else {
            return BTreeSet::new();
        };
        let guard = state.read().await;
        let Some(cursors) = guard.blob_cursors() else {
            return BTreeSet::new();
        };
        request
            .record
            .attachments
            .iter()
            .filter(|attachment| {
                cursors
                    .store
                    .has_named_holder(&attachment.blob_digest, holder.as_str())
                    .unwrap_or(false)
            })
            .map(|attachment| attachment.blob_digest.clone())
            .collect()
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, owner_scope);
        BTreeSet::new()
    }
}

/// Commit the vector write of a remembered record through the existing
/// `AddEmbedding` gateway path.
async fn commit_embedding(ctx: &MutationCtx<'_>, write: &Method) -> Option<Response> {
    let Method::AddEmbedding { node_id, embedding } = write else {
        return None;
    };
    let (node_id, embedding) = (node_id.clone(), embedding.clone());
    let scope = super::gateway_graph::EmbeddingWriteScope {
        tenant: ctx.tenant_scope.to_string(),
        graph: ctx.graph_name.to_string(),
        shard: ctx.persistence.map_or(0, |p| {
            p.shard_of(&medulla_kernel::persist::sanitize(ctx.graph_name))
        }) as u32,
    };
    let plan = MutationPlan::for_method(write);
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, write, move |core| {
            super::gateway_graph::apply_add_embedding(core, node_id, embedding, &scope)
        })
        .await,
    )
}

/// The `Route::MemorySurface` entry: admit the remember, commit the node upsert
/// through the coalescable gateway, then the vector beside it.
pub(super) async fn handle_remember(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
    known_digests: &BTreeSet<String>,
) -> Option<Response> {
    let Method::MemoryRemember { request } = method else {
        return None;
    };
    let ok = |answer| ResultPayload::of::<MemoryRememberResultKind>(answer);
    match admit_remember(ctx.core, ctx.tenant_scope, request, known_digests) {
        Err(error) => Some(Response::err(ctx.req_id, error)),
        Ok(Admitted::Same(answer)) => Some(Response::ok(ctx.req_id, ok(answer))),
        Ok(Admitted::Embed(embed, answer)) => {
            let response = commit_embedding(ctx, &embed).await?;
            Some(if response.error.is_some() {
                response
            } else {
                Response::ok(ctx.req_id, ok(answer))
            })
        }
        Ok(Admitted::Write(write, embed, answer)) => {
            let plan = MutationPlan::for_method(&write);
            let owned = write.as_ref().clone();
            let node = super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
                medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
                ResultPayload::of::<MemoryRememberResultKind>(answer)
            })
            .await;
            let Some(embed) = embed else {
                return Some(node);
            };
            if node.error.is_some() {
                return Some(node);
            }
            let vector = commit_embedding(ctx, &embed).await?;
            Some(if vector.error.is_some() { vector } else { node })
        }
    }
}

/// The derivation text of every attachment of `record`, lower case.
fn derivation_text(core: &GraphCore, record: &MemoryRecord) -> String {
    record
        .attachments
        .iter()
        .flat_map(|attachment| attachment.derivation_ids.iter())
        .filter_map(|id| core.get_node_properties(id.as_str()))
        .filter_map(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .filter_map(|props| {
            props
                .get(DERIVATION_TEXT_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_lowercase)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The share of `keywords` found in the record's text, topics or derivation text.
fn keyword_overlap(core: &GraphCore, record: &MemoryRecord, keywords: &[String]) -> f64 {
    if keywords.is_empty() {
        return 0.0;
    }
    let haystack = format!(
        "{} {} {}",
        record.text.to_lowercase(),
        record.topics.join(" ").to_lowercase(),
        derivation_text(core, record)
    );
    let found = keywords
        .iter()
        .filter(|keyword| haystack.contains(keyword.as_str()))
        .count();
    found as f64 / keywords.len() as f64
}

fn to_q16(score: f64) -> u32 {
    (score.clamp(0.0, 1.0) * Q16_ONE).round() as u32
}

fn invalid(detail: impl std::fmt::Display) -> String {
    refuse("PARAMETER_INVALID", detail)
}

/// A record whose attachment derivation scores above the record's own cosine takes that score as
/// its candidate similarity; a record the vector index did not return joins at that score. The
/// record stays one candidate (MEDULLA-MEDIA-VOCAB-R650.3).
fn raise_by_derivations(
    core: &GraphCore,
    tenant: &str,
    request: &MemoryRecallRequest,
    searched_by_request: &impl Fn(&Stored) -> bool,
    candidates: &mut Vec<(f64, Stored)>,
) {
    let tiers: &[MemoryTier] = if request.tiers.is_empty() {
        &TIERS
    } else {
        &request.tiers
    };
    for stored in load(core, tenant, tiers) {
        if !searched_by_request(&stored) {
            continue;
        }
        let owned: Vec<(String, Vec<f32>)> = {
            let store = core.semantic_store.read();
            stored
                .record
                .attachments
                .iter()
                .flat_map(|attachment| attachment.derivation_ids.iter())
                .filter_map(|id| {
                    store
                        .get_embedding(id.as_str())
                        .map(|embedding| (id.as_str().to_string(), embedding))
                })
                .collect()
        };
        let Some(best) =
            match_derivation(&request.query_embedding, &owned, SEARCH_FANOUT).into_iter().next()
        else {
            continue;
        };
        let score = f64::from(best.score).clamp(0.0, 1.0);
        let held = candidates
            .iter()
            .position(|(_, held)| held.record.memory_id == stored.record.memory_id);
        match held {
            Some(at) if score > candidates[at].0 => candidates[at].0 = score,
            Some(_) => {}
            None => candidates.push((score, stored)),
        }
    }
}

/// Answer one recall for `tenant` as of `now_ms`.
pub(super) fn recall(
    core: &GraphCore,
    tenant: &str,
    now_ms: i64,
    request: &MemoryRecallRequest,
) -> Result<MemoryRecallResult, String> {
    check_embedding(&request.query_embedding, "the query embedding")?;
    if request.budget == 0 {
        return Err(invalid("a recall needs a token budget above zero"));
    }
    let keywords: Vec<String> = request
        .keywords
        .iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    let has_embedding = !request.query_embedding.is_empty();
    let has_keywords = !keywords.is_empty();
    if request.include_media && !has_embedding {
        return Err(invalid("include_media needs a non-empty query embedding"));
    }
    if !has_embedding && !has_keywords && request.prime_session.is_none() && !request.include_plan
    {
        return Err(invalid(
            "a recall needs a query embedding, keywords, a session to prime or a plan",
        ));
    }
    let as_of = request.as_of_ms.unwrap_or(now_ms);
    let tiers: &[MemoryTier] = if request.tiers.is_empty() {
        &TIERS
    } else {
        &request.tiers
    };
    let visible = |stored: &Stored| {
        let record = &stored.record;
        !record.forgotten
            && record.timestamp_ms <= as_of
            && record.expires_at_ms.is_none_or(|expires| expires > as_of)
    };
    let searched_by_request = |stored: &Stored| {
        visible(stored)
            && tiers.contains(&stored.record.tier)
            && request
                .scope
                .as_ref()
                .is_none_or(|scope| stored.record.session_id.as_ref() == Some(scope))
    };

    // Candidates: cosine on the vector index when the query carries an
    // embedding, otherwise a bounded scan of the tiers for the keywords.
    let mut candidates: Vec<(f64, Stored)> = Vec::new();
    if has_embedding {
        let prefix = tenant_prefix(tenant);
        let found = core.semantic_store.read().semantic_search_filtered(
            &request.query_embedding,
            SEARCH_FANOUT,
            |id| id.starts_with(&prefix),
        );
        for (id, similarity) in found {
            let Some(stored) = core
                .get_node_properties(&id)
                .and_then(|blob| decode(&blob, tenant))
            else {
                continue;
            };
            if searched_by_request(&stored) {
                candidates.push((f64::from(similarity).clamp(0.0, 1.0), stored));
            }
        }
        if request.include_media {
            raise_by_derivations(core, tenant, request, &searched_by_request, &mut candidates);
        }
    } else if has_keywords {
        candidates = load(core, tenant, tiers)
            .into_iter()
            .filter(|stored| searched_by_request(stored))
            .map(|stored| (0.0, stored))
            .collect();
    }
    let mut ranked: Vec<(f64, Stored)> = candidates
        .into_iter()
        .map(|(semantic, stored)| {
            let lexical = keyword_overlap(core, &stored.record, &keywords);
            let score = match (has_embedding, has_keywords) {
                (true, true) => SEMANTIC_WEIGHT * semantic + KEYWORD_WEIGHT * lexical,
                (true, false) => semantic,
                _ => lexical,
            };
            (score, stored)
        })
        .filter(|(score, _)| *score > 0.0)
        .collect();
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.record.memory_id.as_str().cmp(b.1.record.memory_id.as_str()))
    });
    let mut hits = Vec::new();
    let mut tokens_used: u32 = 0;
    for (score, stored) in ranked {
        let cost = kernel::record_tokens(&stored.record);
        if tokens_used.saturating_add(cost) > request.budget {
            continue;
        }
        tokens_used += cost;
        hits.push(MemoryHit {
            record: stored.record,
            score_q16: to_q16(score),
        });
    }

    let mementos = match &request.prime_session {
        None => Vec::new(),
        Some(session) => {
            let records: Vec<MemoryRecord> = load(core, tenant, &TIERS)
                .into_iter()
                .filter(visible)
                .filter(|stored| stored.record.session_id.as_ref() == Some(session))
                .map(|stored| stored.record)
                .collect();
            let cap = u32::try_from(knob_integer(&kernel::MEMENTO_TOKEN_CAP)).unwrap_or(u32::MAX);
            kernel::compress_to_mementos(&records, cap)
        }
    };

    let plan = request.include_plan.then(|| {
        let searched: Vec<MemoryRecord> = load(core, tenant, tiers)
            .into_iter()
            .filter(|stored| searched_by_request(stored))
            .map(|stored| stored.record)
            .collect();
        plan_over(core, tenant, &searched)
    });
    Ok(MemoryRecallResult {
        hits,
        tokens_used,
        mementos,
        plan,
    })
}

/// The consolidation plan over the searched records. Each record's nearest
/// neighbours come from the vector index, restricted to the tenant's memory
/// nodes; the kernel only reads them.
fn plan_over(
    core: &GraphCore,
    tenant: &str,
    searched: &[MemoryRecord],
) -> medulla_work_types::memory::ConsolidationPlan {
    let covered = &searched[..searched.len().min(PLAN_RECORD_CAP)];
    let merge_cosine = knob_decimal(&kernel::MERGE_COSINE);
    let prefix = tenant_prefix(tenant);
    let by_id: BTreeMap<&str, &MemoryRecord> = covered
        .iter()
        .map(|record| (record.memory_id.as_str(), record))
        .collect();
    let mut neighbours: BTreeMap<ResourceId, Vec<(ResourceId, f64)>> = BTreeMap::new();
    for record in covered {
        let id = node_id(tenant, record.memory_id.as_str());
        let Some(embedding) = core.semantic_store.read().get_embedding(&id) else {
            continue;
        };
        let found = core.semantic_store.read().semantic_search_filtered(
            &embedding,
            NEIGHBOURS_PER_RECORD + 1,
            |candidate| candidate.starts_with(&prefix),
        );
        let near: Vec<(ResourceId, f64)> = found
            .into_iter()
            .filter_map(|(candidate, similarity)| {
                let memory_id = candidate.strip_prefix(&prefix)?;
                let other = by_id.get(memory_id)?;
                (other.memory_id != record.memory_id && other.tier == record.tier)
                    .then(|| (other.memory_id.clone(), f64::from(similarity)))
            })
            .filter(|(_, similarity)| *similarity >= merge_cosine)
            .collect();
        if !near.is_empty() {
            neighbours.insert(record.memory_id.clone(), near);
        }
    }
    kernel::plan_consolidation(
        covered,
        &neighbours,
        merge_cosine,
        knob_decimal(&kernel::IMPORTANCE_FLOOR),
    )
}

/// Answer one export for `tenant`.
pub(super) fn export(
    core: &GraphCore,
    tenant: &str,
    request: &MemoryExportRequest,
) -> Result<MemoryExportResult, String> {
    let (min, max) = knob_bounds(&kernel::EXPORT_MAX_RECORDS);
    let limit = request
        .limit
        .map_or(knob_integer(&kernel::EXPORT_MAX_RECORDS), i64::from)
        .clamp(min, max);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut records: Vec<MemoryRecord> = load(core, tenant, &EXPORT_TIERS)
        .into_iter()
        .filter(|stored| !stored.record.forgotten)
        .map(|stored| stored.record)
        .collect();
    let truncated = records.len() > limit;
    records.truncate(limit);
    let mut hasher = Sha256::new();
    for record in &records {
        hasher.update(record.memory_id.as_str().as_bytes());
        hasher.update(b"\n");
    }
    let ids_digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(MemoryExportResult {
        manifest: MemoryExportManifest {
            tiers: EXPORT_TIERS.to_vec(),
            record_count: u32::try_from(records.len()).unwrap_or(u32::MAX),
            ids_digest,
            truncated,
        },
        records,
    })
}

/// Route the memory reads served from the graph.
pub(super) async fn try_handle_memory_surface(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if !matches!(
        method,
        Method::MemoryRecall { .. } | Method::MemoryExport { .. }
    ) {
        return ControlFlow::Continue(method);
    }
    let Some(tenant) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse("ACCESS_DENIED", "a memory read needs a verified tenant"),
        ));
    };
    let answer = match &method {
        Method::MemoryRecall { request } => {
            let now_ms = i64::try_from(authoritative_now_ms()).unwrap_or(i64::MAX);
            recall(ctx.core, &tenant, now_ms, request)
                .and_then(ResultPayload::of::<MemoryRecallResultKind>)
        }
        Method::MemoryExport { request } => {
            export(ctx.core, &tenant, request).and_then(ResultPayload::of::<MemoryExportResultKind>)
        }
        _ => unreachable!("only memory reads reach this arm"),
    };
    ControlFlow::Break(match answer {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    })
}
