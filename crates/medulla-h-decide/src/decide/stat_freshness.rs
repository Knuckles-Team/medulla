//! Fresh data through the search connector's virtual-graph source
//! (MEDULLA-DECISION-ENGINE-R174.9.3 to R174.9.5): `DecisionLog.learn` with a
//! `RefreshFreshness` write reads the logical graph's registered search source
//! through a [`SearchSourcePort`], deduplicates every result by canonical URL
//! and content digest, stores the new and superseding rows with their
//! provenance, marks one region per changed row, and writes the receipt.
//!
//! The read goes through the virtual-graph source; no extract-transform-load
//! copy exists. Every refusal is decided before the first read, so a refused
//! refresh writes no row, no region mark and no receipt.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use medulla_decision_types::decision::statistical::retrieval_refresh::{
    canonical_url, classify_result, BudgetRefusal, DedupeOutcome, FreshResult, FreshnessReceipt,
    FreshnessRefresh, RefreshSpend, RegionMark, StoredFreshRow, FRESHNESS_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use sha2::{Digest, Sha256};

use super::stat_executor::ExecutionContext;
use super::stat_support::refusal;

/// Key family of held rows: `freshness:row:<graph>:<canonical url>`.
const ROW_PREFIX: &str = "freshness:row:";
/// Key family of receipts: `freshness:receipt:<refresh id>`.
const RECEIPT_PREFIX: &str = "freshness:receipt:";
/// Key of a graph's region revision counter.
const REVISION_PREFIX: &str = "freshness:revision:";
/// The node type a refreshed row belongs to.
const NODE_TYPE: &str = "Document";
/// Shards a graph's refreshed rows spread over.
pub(super) const REGION_SHARDS: u32 = 64;
/// Most held rows one refresh reads back to deduplicate against.
const MAX_HELD_ROWS: usize = 100_000;

/// One search result as the source returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub engine: String,
    /// Unix seconds the source retrieved it.
    pub retrieved_at_s: i64,
}

/// What one query returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHits {
    /// SHA-256 of the pushed arguments, 64 lowercase hex characters.
    pub query_digest: String,
    pub hits: Vec<SearchHit>,
}

/// The registered search source of one logical graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSourceBinding {
    /// The `vg:SourceConnection` the results are read through.
    pub connection_iri: String,
    /// Cost units one request spends.
    pub cost_units_per_request: u64,
}

/// The virtual-graph read seam of a refresh: the registered `searxng`
/// `vg:VirtualMapping` reached through its `SourceConnection`.
pub trait SearchSourcePort: Send + Sync {
    /// The search source `tenant`'s `graph` has registered, if any.
    fn binding(&self, tenant: &str, graph: &str) -> Option<SearchSourceBinding>;
    /// Read `query` through the source at `now_ms`.
    fn read(
        &self,
        binding: &SearchSourceBinding,
        query: &str,
        now_ms: u64,
    ) -> Result<SearchHits, String>;
}

static PORT: RwLock<Option<Arc<dyn SearchSourcePort>>> = RwLock::new(None);

/// Register the host's search source port, replacing any earlier one.
pub fn register_search_source_port(port: Arc<dyn SearchSourcePort>) {
    if let Ok(mut slot) = PORT.write() {
        *slot = Some(port);
    }
}

/// Drop the registered port.
pub fn clear_search_source_port() {
    if let Ok(mut slot) = PORT.write() {
        *slot = None;
    }
}

fn registered_port() -> Option<Arc<dyn SearchSourcePort>> {
    PORT.read().ok().and_then(|slot| slot.clone())
}

/// Request spending per tenant and graph: the token bucket over
/// `freshness.window`, kept while the process runs.
static SPENDING: Mutex<BTreeMap<(String, String), RefreshSpend>> = Mutex::new(BTreeMap::new());

fn sha256_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// The content digest of a result: SHA-256 over its title, snippet and
/// engine, each length-framed. The search source returns no page body, so the
/// content a result carries is its title and snippet.
pub(super) fn content_digest(hit: &SearchHit) -> String {
    sha256_hex(&[&hit.title, &hit.snippet, &hit.engine])
}

/// The region (shard) a canonical URL's row belongs to: its host, hashed.
pub(super) fn region_shard(canonical: &str) -> u32 {
    let host = canonical
        .split_once("://")
        .map_or(canonical, |(_, rest)| rest)
        .split(['/', '?'])
        .next()
        .unwrap_or_default();
    let digest = Sha256::digest(host.as_bytes());
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % REGION_SHARDS
}

fn row_key(graph: &str, canonical: &str) -> String {
    format!("{ROW_PREFIX}{graph}:{canonical}")
}

fn revision_key(graph: &str) -> String {
    format!("{REVISION_PREFIX}{graph}")
}

/// The id of a refresh: its graph, trigger, queries and the time it was asked.
fn refresh_id(request: &FreshnessRefresh, now_ms: u64, sequence: u64) -> String {
    let queries = request.queries.as_slice().join("\u{1f}");
    sha256_hex(&[
        "freshness-refresh/v1",
        &request.logical_graph,
        &queries,
        &now_ms.to_string(),
        &sequence.to_string(),
    ])
}

/// The held digest of every canonical URL a graph holds.
fn held_digests(ctx: &ExecutionContext, graph: &str) -> Result<BTreeMap<String, String>, String> {
    let prefix = format!("{ROW_PREFIX}{graph}:");
    let mut held = BTreeMap::new();
    for (_, bytes) in
        ctx.store
            .decision_artifacts_with_prefix(ctx.tenant_id, &prefix, MAX_HELD_ROWS)?
    {
        let row: StoredFreshRow = decode_artifact(&bytes, "freshness row")?;
        held.insert(row.canonical_url, row.result.content_digest);
    }
    Ok(held)
}

/// Every row `graph` holds, by canonical URL.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn held_rows(
    ctx: &ExecutionContext,
    graph: &str,
) -> Result<BTreeMap<String, StoredFreshRow>, String> {
    let prefix = format!("{ROW_PREFIX}{graph}:");
    let mut rows = BTreeMap::new();
    for (_, bytes) in
        ctx.store
            .decision_artifacts_with_prefix(ctx.tenant_id, &prefix, MAX_HELD_ROWS)?
    {
        let row: StoredFreshRow = decode_artifact(&bytes, "freshness row")?;
        rows.insert(row.canonical_url.clone(), row);
    }
    Ok(rows)
}

/// The receipt of refresh `refresh_id`, if this tenant ran it.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn stored_receipt(
    ctx: &ExecutionContext,
    refresh_id: &str,
) -> Result<Option<FreshnessReceipt>, String> {
    ctx.store
        .decision_artifact(ctx.tenant_id, &format!("{RECEIPT_PREFIX}{refresh_id}"))?
        .map(|bytes| decode_artifact(&bytes, "freshness receipt"))
        .transpose()
}

/// A graph's counters: region marks written and refreshes served so far.
fn counters(ctx: &ExecutionContext, graph: &str) -> Result<(u64, u64), String> {
    Ok(ctx
        .store
        .decision_artifact(ctx.tenant_id, &revision_key(graph))?
        .map(|bytes| decode_artifact::<(u64, u64)>(&bytes, "freshness counters"))
        .transpose()?
        .unwrap_or((0, 0)))
}

/// The region marks written for `graph` so far: its revision counter.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn region_revision(ctx: &ExecutionContext, graph: &str) -> Result<u64, String> {
    Ok(counters(ctx, graph)?.0)
}

/// Check the request's policies and, with the source's declared cost, its
/// budget. Nothing is read or written here.
fn admit(
    ctx: &ExecutionContext,
    request: &FreshnessRefresh,
    binding: &SearchSourceBinding,
) -> Result<RefreshSpend, String> {
    if request.acl_policy_ref.trim().is_empty() {
        return Err(refusal(
            StatisticalErrorCode::RefreshAclPolicyMissing,
            "a refresh names the ACL policy its rows are read under",
        ));
    }
    if request.privacy_policy_ref.trim().is_empty() {
        return Err(refusal(
            StatisticalErrorCode::RefreshPrivacyPolicyMissing,
            "a refresh names the privacy policy its rows are held under",
        ));
    }
    let window_ms = medulla_server_api::knob_config::knobs()
        .freshness_window
        .saturating_mul(1_000);
    let key = (ctx.tenant_id.to_string(), request.logical_graph.clone());
    let mut spend = SPENDING
        .lock()
        .map_err(|_| "the refresh spending ledger is poisoned".to_string())?
        .get(&key)
        .filter(|held| held.same_budget(&request.budget))
        .cloned()
        .unwrap_or_else(|| RefreshSpend::new(&request.budget, window_ms, ctx.now_ms));
    for _ in request.queries.iter() {
        spend
            .charge(binding.cost_units_per_request, ctx.now_ms)
            .map_err(|which| match which {
                BudgetRefusal::Requests => refusal(
                    StatisticalErrorCode::RefreshRateBudgetExceeded,
                    format!(
                        "more than {} requests in the window",
                        request.budget.max_requests
                    ),
                ),
                BudgetRefusal::Cost => refusal(
                    StatisticalErrorCode::RefreshCostBudgetExceeded,
                    format!("more than {} cost units", request.budget.max_cost_units),
                ),
            })?;
    }
    Ok(spend)
}

/// Serve one refresh through `port`.
pub(super) fn refresh_with(
    ctx: &ExecutionContext,
    port: &dyn SearchSourcePort,
    request: &FreshnessRefresh,
) -> Result<FreshnessReceipt, String> {
    request
        .check()
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    let graph = request.logical_graph.as_str();
    let binding = port.binding(ctx.tenant_id, graph).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::SearchSourceUnregistered,
            format!("graph `{graph}` has no registered search source"),
        )
    })?;
    let spend = admit(ctx, request, &binding)?;

    let (mut revision, sequence) = counters(ctx, graph)?;
    let id = refresh_id(request, ctx.now_ms, sequence + 1);
    let mut held = held_digests(ctx, graph)?;
    let mut receipt = FreshnessReceipt {
        schema_version: FRESHNESS_SCHEMA_VERSION,
        refresh_id: id.clone(),
        logical_graph: graph.to_string(),
        source_connection_iri: binding.connection_iri.clone(),
        trigger: request.trigger,
        new_count: 0,
        unchanged_count: 0,
        superseded_count: 0,
        requests_spent: request.queries.len() as u32,
        cost_units_spent: binding
            .cost_units_per_request
            .saturating_mul(request.queries.len() as u64),
        regions: Vec::new(),
        stored: Vec::new(),
        completed_at_ms: ctx.now_ms,
    };
    let mut writes: Vec<(String, Vec<u8>)> = Vec::new();
    for query in request.queries.iter() {
        let read = port.read(&binding, query, ctx.now_ms)?;
        for hit in &read.hits {
            let canonical = canonical_url(&hit.url)?;
            let digest = content_digest(hit);
            let outcome = classify_result(held.get(&canonical).map(String::as_str), &digest);
            let supersedes = match &outcome {
                DedupeOutcome::New => {
                    receipt.new_count += 1;
                    None
                }
                DedupeOutcome::Unchanged => {
                    receipt.unchanged_count += 1;
                    continue;
                }
                DedupeOutcome::Superseded { previous_digest } => {
                    receipt.superseded_count += 1;
                    Some(previous_digest.clone())
                }
            };
            let result = FreshResult {
                schema_version: FRESHNESS_SCHEMA_VERSION,
                url: canonical.clone(),
                content_digest: digest.clone(),
                source_connection_iri: binding.connection_iri.clone(),
                retrieved_at: u64::try_from(hit.retrieved_at_s)
                    .unwrap_or_default()
                    .saturating_mul(1_000),
                query_text: query.clone(),
                query_digest: read.query_digest.clone(),
                refresh_id: id.clone(),
            };
            result
                .check()
                .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
            held.insert(canonical.clone(), digest);
            revision += 1;
            receipt.regions.push(RegionMark {
                shard_id: region_shard(&canonical),
                node_type: NODE_TYPE.to_string(),
                revision,
            });
            writes.push((
                row_key(graph, &canonical),
                encode_artifact(&StoredFreshRow {
                    schema_version: FRESHNESS_SCHEMA_VERSION,
                    logical_graph: graph.to_string(),
                    canonical_url: canonical,
                    result: result.clone(),
                    supersedes,
                })?,
            ));
            receipt.stored.push(result);
        }
    }
    writes.push((
        revision_key(graph),
        encode_artifact(&(revision, sequence + 1))?,
    ));
    writes.push((format!("{RECEIPT_PREFIX}{id}"), encode_artifact(&receipt)?));
    ctx.store.put_decision_artifacts(ctx.tenant_id, &writes)?;
    SPENDING
        .lock()
        .map_err(|_| "the refresh spending ledger is poisoned".to_string())?
        .insert((ctx.tenant_id.to_string(), graph.to_string()), spend);
    Ok(receipt)
}

/// Serve one refresh through the registered port.
pub(super) fn refresh(
    ctx: &ExecutionContext,
    request: &FreshnessRefresh,
) -> Result<FreshnessReceipt, String> {
    let port = registered_port().ok_or_else(|| {
        refusal(
            StatisticalErrorCode::SearchSourceUnregistered,
            "no search source port is registered",
        )
    })?;
    refresh_with(ctx, port.as_ref(), request)
}
