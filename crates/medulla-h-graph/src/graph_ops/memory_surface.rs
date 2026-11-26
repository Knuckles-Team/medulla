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
use medulla_decision_types::decision::knob_families::memory::{ENTRY_MAX_BYTES, PROMOTE_MIN_SUPPORT, TENANT_MAX_BYTES};
use medulla_decision_types::decision::knobs::{KnobSpec, KnobValue};
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::graph::{
    MemoryExport as MemoryExportResultKind, MemoryRecall as MemoryRecallResultKind,
    MemoryRemember as MemoryRememberResultKind,
};
use medulla_modality::media_memory::match_derivation;
use medulla_types::contract::ResourceId;
use medulla_work_types::memory::{
    MemoryDriftReport, MemoryExportManifest, MemoryExportRequest, MemoryExportResult, MemoryHit, MemoryRecallRequest,
    MemoryRecallResult, MemoryRecord, MemoryRememberRequest, MemoryRememberResult, MemoryTier,
    MEMORY_ENTRY_TOO_LARGE, MEMORY_RECORD_SCHEMA_VERSION, MEMORY_SUPPORT_BELOW_THRESHOLD,
    MEMORY_TENANT_QUOTA_EXCEEDED,
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

/// The refusal of a promotion group below `knob memory.promote_min_support`
/// (MEDULLA-DECISION-ENGINE-R651.7). A `Consolidate` request whose semantic
/// properties name a `pattern_key` is a promotion group, and its support is the
/// number of distinct episode ids it sends. A consolidation without a
/// `pattern_key` is a plain consolidation and is never counted.
pub(super) fn promotion_support_refusal(
    episodic_ids: &[String],
    semantic_props: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let names_pattern = semantic_props
        .get("pattern_key")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.is_empty());
    if !names_pattern {
        return None;
    }
    let support = episodic_ids.iter().collect::<BTreeSet<_>>().len();
    let required = usize::try_from(knob_integer(&PROMOTE_MIN_SUPPORT)).unwrap_or(usize::MAX);
    (support < required).then(|| {
        refuse(
            MEMORY_SUPPORT_BELOW_THRESHOLD.as_str(),
            format!(
                "a promotion group needs at least {required} episodes (knob memory.promote_min_support), got {support}"
            ),
        )
    })
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

/// The node type of a tenant's memory usage counter (MEDULLA-DECISION-ENGINE-R154.1).
const USAGE_NODE_TYPE: &str = "MemoryTenantUsage";

/// The id of the durable counter node of `tenant`: the stored bytes of its memory nodes.
fn usage_node_id(tenant: &str) -> String {
    format!("memory-quota:{tenant}")
}

/// The tenant's counter, when its node is stored.
fn read_usage(core: &GraphCore, tenant: &str) -> Option<u64> {
    let blob = core.get_node_properties(&usage_node_id(tenant))?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    if props.get(TENANT_KEY)?.as_str()? != tenant {
        return None;
    }
    props.get("bytes")?.as_u64()
}

/// The stored bytes of every memory node of `tenant`, measured by scanning the four tiers.
fn scan_usage(core: &GraphCore, tenant: &str) -> u64 {
    TIERS
        .iter()
        .flat_map(|tier| core.get_nodes_by_label(tier.node_type(), SCAN_CAP_PER_TIER))
        .filter(|(_, blob)| decode(blob, tenant).is_some())
        .map(|(_, blob)| blob.len() as u64)
        .sum()
}

/// The counter node properties holding `bytes`.
fn usage_props(tenant: &str, bytes: u64) -> serde_json::Value {
    serde_json::json!({"type": USAGE_NODE_TYPE, TENANT_KEY: tenant, "bytes": bytes})
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

/// The one answer for a memory record the caller cannot see, whether it is absent or another
/// tenant's: the same code and the same message, which never mentions a tenant.
pub(super) fn absent_record(memory_id: &str) -> String {
    refuse(
        "PARAMETER_INVALID",
        format!("memory record {memory_id} does not exist"),
    )
}

fn node_props(tenant: &str, request: &MemoryRememberRequest) -> serde_json::Value {
    let mut props = request.record.to_props();
    props.insert(TENANT_KEY.into(), tenant.into());
    serde_json::Value::Object(props)
}

fn node_blob(tenant: &str, request: &MemoryRememberRequest) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(&node_props(tenant, request)).map_err(|error| {
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
    /// The one batch to commit (the record node and the tenant's usage counter node, written
    /// together), then the vector write beside it, and the answer.
    Write(Box<Method>, Option<Box<Method>>, MemoryRememberResult),
    /// The node is already stored but its vector is not indexed: write the vector only.
    Embed(Box<Method>, MemoryRememberResult),
    /// The very same record and vector are already stored: nothing to commit.
    Same(MemoryRememberResult),
}

/// `knob memory.tenant_max_bytes` (MEDULLA-DECISION-ENGINE-R154.1).
pub(super) fn tenant_max_bytes() -> u64 {
    u64::try_from(knob_integer(&TENANT_MAX_BYTES)).unwrap_or(u64::MAX)
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
    tenant_max_bytes: u64,
) -> Result<Admitted, String> {
    let record = &request.record;
    // A record naming another tenant answers exactly like a record that does not exist for
    // the caller (MEDULLA-DECISION-ENGINE-R651.4): no distinct code, no existence oracle.
    if record.tenant.as_str() != tenant {
        return Err(absent_record(record.memory_id.as_str()));
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
    // MEDULLA-DECISION-ENGINE-R651.4: the bytes the node would store, measured before any write.
    let blob = node_blob(tenant, request)?;
    let max_entry_bytes = usize::try_from(knob_integer(&ENTRY_MAX_BYTES)).unwrap_or(usize::MAX);
    if blob.len() > max_entry_bytes {
        return Err(refuse(
            MEMORY_ENTRY_TOO_LARGE.as_str(),
            format!(
                "memory record stores {} bytes, above knob memory.entry_max_bytes ({max_entry_bytes})",
                blob.len()
            ),
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
    let max_blob_bytes = medulla_work_types::memory::media_max_blob_bytes();
    if let Some(attachment) = record
        .attachments
        .iter()
        .find(|attachment| attachment.byte_len > max_blob_bytes)
    {
        return Err(refuse(
            "MEDIA_TOO_LARGE",
            format!(
                "attachment of {} bytes is above knob media.max_blob_bytes ({max_blob_bytes})",
                attachment.byte_len
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
        let write = admitted_batch(core, tenant, &id, request, (blob.len() as u64, tenant_max_bytes))?;
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

/// Admit the tenant total (MEDULLA-DECISION-ENGINE-R154.1) and build the one batch that writes the
/// record node and the tenant's counter node. The counter is a durable graph node; the batch is
/// the only writer of a new record, so the counter and the node land together. A projected total
/// above `knob memory.tenant_max_bytes` refuses `MEMORY_TENANT_QUOTA_EXCEEDED`. The counter is
/// exact: every write and removal of a memory node moves it in the same transaction
/// (`GraphTxn::add_node` / `remove_node`), so admission trusts it and never re-measures. A tenant
/// with no counter node yet is measured once by a scan, and the batch below writes its counter.
fn admitted_batch(
    core: &GraphCore,
    tenant: &str,
    id: &str,
    request: &MemoryRememberRequest,
    (blob_len, max_tenant): (u64, u64),
) -> Result<Method, String> {
    let replaced = core.get_node_properties(id).map_or(0, |stored| stored.len() as u64);
    let project = |total: u64| total.saturating_sub(replaced).saturating_add(blob_len);
    let projected = project(read_usage(core, tenant).unwrap_or_else(|| scan_usage(core, tenant)));
    if projected > max_tenant {
        return Err(refuse(
            MEMORY_TENANT_QUOTA_EXCEEDED.as_str(),
            format!(
                "the tenant's memory would store {projected} bytes, above knob memory.tenant_max_bytes ({max_tenant})"
            ),
        ));
    }
    let operations = serde_json::json!([
        {"op": "add_node", "id": id, "properties": node_props(tenant, request)},
        {"op": "add_node", "id": usage_node_id(tenant), "properties": usage_props(tenant, projected)},
    ]);
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| refuse("PARAMETER_INVALID", format!("record encoding failed: {error}")))?;
    Ok(Method::BatchUpdate { operations_msgpack })
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

/// Check each held attachment's declared MIME against the type its first bytes name
/// (MEDULLA-MEDIA-VOCAB-R650.4). An attachment the owner scope does not hold, or one above the
/// size limit, is left to `admit_remember`, which refuses it first. `Ok` for any other method.
pub(super) async fn check_attachment_types(
    state: &Arc<RwLock<ServerState>>,
    method: &Method,
    known_digests: &BTreeSet<String>,
) -> Result<(), String> {
    let Method::MemoryRemember { request } = method else {
        return Ok(());
    };
    #[cfg(feature = "blob")]
    {
        use medulla_server_blob::blob::store::ChunkStore;
        use medulla_server_blob::blob::BlobStateExt;
        let max_blob_bytes = medulla_work_types::memory::media_max_blob_bytes();
        let store = {
            let guard = state.read().await;
            guard.blob_cursors().map(|cursors| cursors.store.clone())
        };
        let Some(store) = store else {
            return Ok(());
        };
        for attachment in &request.record.attachments {
            if !known_digests.contains(&attachment.blob_digest)
                || attachment.byte_len > max_blob_bytes
            {
                continue;
            }
            let Some(chunk) = store.get_chunk(&attachment.blob_digest)? else {
                continue;
            };
            match medulla_modality::media_memory::sniff_mime(&chunk) {
                None => {
                    return Err(refuse(
                        "MEDIA_TYPE_UNSUPPORTED",
                        format!("attachment '{}' has an unsupported type", attachment.blob_digest),
                    ));
                }
                Some(sniffed) if sniffed != attachment.mime => {
                    return Err(refuse(
                        "MEDIA_TYPE_MISMATCH",
                        format!(
                            "attachment '{}' declares '{}' but is '{sniffed}'",
                            attachment.blob_digest, attachment.mime
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, request, known_digests);
        Ok(())
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

/// The per-tenant gate serializing the remembers of one tenant.
fn tenant_gate(tenant: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static GATES: std::sync::OnceLock<
        std::sync::Mutex<BTreeMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let mut gates = GATES
        .get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::sync::Arc::clone(gates.entry(tenant.to_string()).or_default())
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
    // One remember of a tenant at a time: the counter node is read at admission and written by
    // the batch, so two concurrent writes must not both start from the same total.
    let tenant_gate = tenant_gate(ctx.tenant_scope);
    let _held = tenant_gate.lock().await;
    match admit_remember(ctx.core, ctx.tenant_scope, request, known_digests, tenant_max_bytes()) {
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
            let Method::BatchUpdate { operations_msgpack } = write.as_ref().clone() else {
                return Some(Response::err(ctx.req_id, refuse("PARAMETER_INVALID", "an admitted write is one batch")));
            };
            let node = medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
                medulla_graph_algos::algorithms::batch_update(core, &operations_msgpack)?;
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
    allowed: Option<&std::collections::BTreeSet<String>>,
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
    // MEDULLA-DECISION-ENGINE-R656.3: a resolved document set only narrows;
    // a record counts when the document it was derived from is a member.
    let in_document_set = |stored: &Stored| {
        allowed.is_none_or(|set| {
            stored
                .record
                .provenance_key
                .as_ref()
                .is_some_and(|key| set.contains(key.as_str()))
        })
    };
    let searched_by_request = |stored: &Stored| {
        visible(stored)
            && in_document_set(stored)
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
    let plan_digest = recall_plan_digest(request, &keywords)?;
    let drift = request
        .previous_manifest_digest
        .as_deref()
        .map(|previous| manifest_drift(core, tenant, previous))
        .transpose()?;
    Ok(MemoryRecallResult {
        hits,
        tokens_used,
        mementos,
        plan,
        plan_digest,
        drift,
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

/// Domain of the manifest digest (MEDULLA-DECISION-ENGINE-R651.5).
const MANIFEST_DIGEST_DOMAIN: &str = "eg/memory-manifest/v1";
/// Domain of the recall plan digest (MEDULLA-DECISION-ENGINE-R651.5).
const RECALL_PLAN_DIGEST_DOMAIN: &str = "eg/memory-recall-plan/v1";

fn hex_digest(domain: &str, canonical: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(canonical);
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 over the canonical JSON of `records` in order, 64 lowercase hex characters.
fn manifest_digest(records: &[MemoryRecord]) -> Result<String, String> {
    let canonical = serde_json::to_vec(records)
        .map_err(|error| refuse("INTERNAL", format!("manifest encoding: {error}")))?;
    Ok(hex_digest(MANIFEST_DIGEST_DOMAIN, &canonical))
}

/// SHA-256 over the canonical recall plan: the query, the filters, the scope
/// and the knob values the recall reads. `previous_manifest_digest` is a
/// comparison input, not part of the plan.
fn recall_plan_digest(request: &MemoryRecallRequest, keywords: &[String]) -> Result<String, String> {
    let plan = serde_json::json!({
        "query_embedding": request.query_embedding,
        "keywords": keywords,
        "tiers": request.tiers,
        "scope": request.scope,
        "as_of_ms": request.as_of_ms,
        "budget": request.budget,
        "prime_session": request.prime_session,
        "include_plan": request.include_plan,
        "include_media": request.include_media,
        "document_set_ids": request.document_set_ids,
        "knobs": {
            "memory.merge_cosine": knob_decimal(&kernel::MERGE_COSINE),
            "memory.memento_token_cap": knob_integer(&kernel::MEMENTO_TOKEN_CAP),
            "memory.importance_floor": knob_decimal(&kernel::IMPORTANCE_FLOOR),
            "memory.export_max_records": knob_integer(&kernel::EXPORT_MAX_RECORDS),
        },
    });
    let canonical = serde_json::to_vec(&plan)
        .map_err(|error| refuse("INTERNAL", format!("plan encoding: {error}")))?;
    Ok(hex_digest(RECALL_PLAN_DIGEST_DOMAIN, &canonical))
}

/// The drift of the caller's current manifest from the one the caller kept.
fn manifest_drift(
    core: &GraphCore,
    tenant: &str,
    previous: &str,
) -> Result<MemoryDriftReport, String> {
    let current = export(core, tenant, &MemoryExportRequest::default())?
        .manifest
        .manifest_digest;
    Ok(MemoryDriftReport {
        cka_q16: None,
        psi_q16: None,
        drifted: current != previous,
        manifest_digest: current,
        previous_manifest_digest: previous.to_owned(),
    })
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
    let manifest_digest = manifest_digest(&records)?;
    Ok(MemoryExportResult {
        manifest: MemoryExportManifest {
            tiers: EXPORT_TIERS.to_vec(),
            record_count: u32::try_from(records.len()).unwrap_or(u32::MAX),
            manifest_digest,
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
            let allowed =
                match super::assistant_profile::resolve_document_sets(&ctx, &request.document_set_ids)
                    .await
                {
                    Ok(allowed) => allowed,
                    Err(error) => return ControlFlow::Break(Response::err(ctx.req_id, error)),
                };
            recall(ctx.core, &tenant, now_ms, request, allowed.as_ref())
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
