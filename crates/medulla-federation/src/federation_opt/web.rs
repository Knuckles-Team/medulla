//! Web metasearch sources (MEDULLA-UNIFIED-DATA-PLANE-R038.7).
//!
//! A pattern over web results becomes one connector call. The planner pushes the query text and
//! the declared `categories`, `engines`, `language` and page as tool arguments; the reply rows
//! are typed results (the URL is the row id), cached per owner by argument digest for the entry
//! TTL, and every row carries its provenance. The engine copies no result into the graph store:
//! results are fetch candidates ([`web_candidates`]), not stored records.
//!
//! Row columns: text `title`, `snippet`, `engine`, `connector`, `argument_digest`, `query_text`;
//! value `rank`, `retrieved_at` (Unix seconds), `from_cache` (1 on a cache hit) and `truncated`
//! (1 when the engine cut results above `max_results`).

use std::collections::BTreeMap;

use medulla_exec::algebra::Op;
use medulla_exec::rowset::{RowSet, TextChannels, ValueChannels};
use medulla_types::web_source::{
    WebCandidateSet, WebResult, WebResultRefusal, WebSearchCapability,
};
use medulla_types::wire::ForeignSourceSpec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::budget::UNSUPPORTED_SEARCH_ARGUMENT;
use super::mcp::McpRemote;
use super::remote::Identity;
use super::search::REMOTE_CHANNEL;
use super::session::FederationSession;
use super::trace::{FetchStrategy, FragmentExecutionMode, FragmentTrace};
use crate::federation::ForeignSourceRegistry;

/// The arguments of one web query: the text and the optional narrowing the tool declares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebQuery {
    pub text: String,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub language: Option<String>,
    /// 1-based result page.
    pub page: u32,
}

impl WebQuery {
    /// A query for `text`, first page.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            page: 1,
            ..Self::default()
        }
    }

    /// The tool arguments, in canonical (sorted) order.
    fn arguments(&self) -> BTreeMap<String, Value> {
        let mut arguments = BTreeMap::new();
        arguments.insert("query".to_string(), json!(self.text));
        arguments.insert("pageno".to_string(), json!(self.page.max(1)));
        if !self.categories.is_empty() {
            arguments.insert("categories".to_string(), json!(self.categories.join(",")));
        }
        if !self.engines.is_empty() {
            arguments.insert("engines".to_string(), json!(self.engines.join(",")));
        }
        if let Some(language) = &self.language {
            arguments.insert("language".to_string(), json!(language));
        }
        arguments
    }

    /// SHA-256 over the canonical arguments, as 64 lowercase hex characters.
    pub fn digest(&self) -> String {
        let canonical = serde_json::to_vec(&self.arguments()).unwrap_or_default();
        hex::encode(Sha256::digest(canonical))
    }

    /// An argument outside the declared capability refuses with `UNSUPPORTED_SEARCH_ARGUMENT`.
    fn check(&self, capability: &WebSearchCapability, label: &str) -> Result<(), String> {
        let outside = |declared: &[String], asked: &[String]| {
            !declared.is_empty() && asked.iter().any(|name| !declared.contains(name))
        };
        let refused = if outside(&capability.categories, &self.categories) {
            Some("categories")
        } else if outside(&capability.engines, &self.engines) {
            Some("engines")
        } else if self.language.as_ref().is_some_and(|language| {
            !capability.languages.is_empty() && !capability.languages.contains(language)
        }) {
            Some("language")
        } else {
            None
        };
        match refused {
            Some(argument) => Err(format!(
                "{UNSUPPORTED_SEARCH_ARGUMENT}: {label}: `{argument}` is outside what the tool declares"
            )),
            None => Ok(()),
        }
    }
}

/// A web metasearch source: an operation-bound `Mcp` source whose tool declares a web capability.
pub(crate) struct WebRemote {
    mcp: McpRemote,
    capability: WebSearchCapability,
}

/// Resolve `spec` (or the spec a registered name holds) to a web source when the bound tool
/// declares a web capability; `None` for every other source.
pub(crate) fn resolve_web(
    spec: &ForeignSourceSpec,
    registry: Option<&ForeignSourceRegistry>,
) -> Option<WebRemote> {
    resolve_web_with(spec, registry, super::mcp::registered_port())
}

/// [`resolve_web`] over an explicit connector port.
pub(crate) fn resolve_web_with(
    spec: &ForeignSourceSpec,
    registry: Option<&ForeignSourceRegistry>,
    port: Option<super::mcp::SharedPort>,
) -> Option<WebRemote> {
    let (spec, name) = match spec {
        ForeignSourceSpec::Named { name } => (registry?.spec(name)?, Some(name.as_str())),
        other => (other, None),
    };
    let ForeignSourceSpec::Mcp {
        server,
        tool,
        capabilities,
    } = spec
    else {
        return None;
    };
    if capabilities.search.is_some() {
        return None;
    }
    let mcp = McpRemote::new(
        (server, tool),
        (capabilities.key_filter_pushdown, None),
        port,
        Identity::of_spec(spec, name),
    );
    let capability = mcp.web_capability()?;
    Some(WebRemote { mcp, capability })
}

/// The entry TTL in milliseconds of a web source, or `None` when `spec` is not one. The served
/// planner calls this for each named source it reads to build the fragment cache scope.
pub fn web_entry_ttl_ms(spec: &ForeignSourceSpec, ttl_s: u64) -> Option<u64> {
    resolve_web(spec, None).map(|_| ttl_s.saturating_mul(1000))
}

/// `Op::ForeignScan` over a web source: push the `RankText` query of the enclosing `FuseRrf`,
/// answer from the cache inside the TTL, and return the results as rows ranked by their rank.
pub(crate) fn web_scan(
    op: &Op,
    input: &RowSet,
    join: bool,
    remote: &WebRemote,
    session: &FederationSession,
) -> Result<RowSet, String> {
    let Some(text) = session.search_text(op) else {
        return Err(format!(
            "{UNSUPPORTED_SEARCH_ARGUMENT}: {}: a web source needs a query; fuse it with a RANK TEXT branch",
            remote.mcp.label()
        ));
    };
    let rows = web_scan_at(
        &WebQuery::new(text),
        remote,
        session,
        super::cache::now_ms(),
    )?;
    if join && !input.is_empty() {
        let keep = input.id_set();
        return Ok(rows.intersect_keep_order(&keep));
    }
    Ok(rows)
}

/// Run `query` against `remote` at wall-clock `now_ms` (a test passes a fake clock).
pub(crate) fn web_scan_at(
    query: &WebQuery,
    remote: &WebRemote,
    session: &FederationSession,
    now_ms: u64,
) -> Result<RowSet, String> {
    let started = std::time::Instant::now();
    let label = remote.mcp.label().to_string();
    query.check(&remote.capability, &label)?;
    let digest = query.digest();
    let mut requests = 0u32;
    let mut fetch = || {
        session.with_meter(|meter| meter.charge_request())?;
        requests += 1;
        let reply = remote.mcp.call(query.arguments(), Vec::new())?;
        let retrieved_at = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX);
        typed_rows(
            &reply,
            query,
            (&label, &digest),
            retrieved_at,
            &remote.capability,
        )
    };
    let scope = session.cache_scope();
    let (rows, from_cache) = match (scope.as_deref(), remote.mcp.cache_name()) {
        (Some(scope), Some(name)) => {
            let fingerprint = remote.mcp.fingerprint();
            let (rows, _stored_at, hit) =
                scope.get_or_fetch_web((name, &fingerprint, &digest), now_ms, fetch)?;
            (rows, hit)
        }
        _ => (fetch()?, false),
    };
    session.with_meter(|meter| meter.charge_rows(rows.len()))?;
    let truncated = rows
        .values()
        .values()
        .any(|columns| columns.get("truncated") == Some(&1.0));
    let out = flag_cache(rows, from_cache);
    let mut trace =
        FragmentTrace::new(label, FetchStrategy::FullFetch, FragmentExecutionMode::Live);
    trace.requests = requests;
    trace.rows_fetched = out.len();
    trace.rows_kept = out.len();
    trace.truncated = truncated;
    trace.read_at_ms = now_ms;
    trace.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    session.record(trace);
    Ok(out)
}

/// Re-set the `from_cache` column of every row.
fn flag_cache(rows: RowSet, from_cache: bool) -> RowSet {
    let flag = if from_cache { 1.0 } else { 0.0 };
    let (kept, mut values) = rows.clone().into_parts();
    for row in &kept {
        values
            .entry(row.id.clone())
            .or_default()
            .insert("from_cache".to_string(), flag);
    }
    rows.with_values(values)
}

/// Validate the provider's rows as web results and attach the engine's provenance columns. More
/// than `max_results` rows are cut and marked `truncated`.
fn typed_rows(
    reply: &RowSet,
    query: &WebQuery,
    (connector, digest): (&str, &str),
    retrieved_at: i64,
    capability: &WebSearchCapability,
) -> Result<RowSet, String> {
    let limit = (capability.max_results as usize).max(1);
    let truncated = reply.len() > limit;
    let (mut ranked, mut values, mut texts) =
        (Vec::new(), ValueChannels::new(), TextChannels::new());
    for (index, row) in reply.rows().iter().take(limit).enumerate() {
        let text = |name: &str| reply.text(&row.id, name).unwrap_or_default().to_string();
        let rank = reply
            .value(&row.id, "rank")
            .map_or(index as u32 + 1, |rank| rank.max(1.0) as u32);
        let result = WebResult {
            url: row.id.clone(),
            title: text("title"),
            snippet: text("snippet"),
            engine: text("engine"),
            rank,
            retrieved_at: reply
                .value(&row.id, "retrieved_at")
                .map_or(retrieved_at, |at| at as i64),
        };
        result
            .validate()
            .map_err(|refusal: WebResultRefusal| refusal.to_string())?;
        let relevance = 1.0 / f64::from(result.rank);
        ranked.push((result.url.clone(), Some(relevance as f32)));
        values.insert(
            result.url.clone(),
            BTreeMap::from([
                ("rank".to_string(), f64::from(result.rank)),
                ("retrieved_at".to_string(), result.retrieved_at as f64),
                ("from_cache".to_string(), 0.0),
                ("truncated".to_string(), if truncated { 1.0 } else { 0.0 }),
                (REMOTE_CHANNEL.to_string(), relevance),
            ]),
        );
        texts.insert(
            result.url.clone(),
            BTreeMap::from([
                ("title".to_string(), result.title),
                ("snippet".to_string(), result.snippet),
                ("engine".to_string(), result.engine),
                ("connector".to_string(), connector.to_string()),
                ("argument_digest".to_string(), digest.to_string()),
                ("query_text".to_string(), query.text.clone()),
            ]),
        );
    }
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(RowSet::from_rows(ranked)
        .with_values(values)
        .with_texts(texts))
}

/// The fetch candidates of one web query's rows (MEDULLA-UNIFIED-DATA-PLANE-R038.7.7): the
/// results with the query text and digest they answer. Nothing is stored.
pub fn web_candidates(rows: &RowSet) -> Result<WebCandidateSet, WebResultRefusal> {
    let first = rows.rows().first().map(|row| row.id.as_str());
    let text_of = |name: &str| {
        first
            .and_then(|id| rows.text(id, name))
            .unwrap_or_default()
            .to_string()
    };
    let results = rows
        .rows()
        .iter()
        .map(|row| WebResult {
            url: row.id.clone(),
            title: rows.text(&row.id, "title").unwrap_or_default().to_string(),
            snippet: rows
                .text(&row.id, "snippet")
                .unwrap_or_default()
                .to_string(),
            engine: rows.text(&row.id, "engine").unwrap_or_default().to_string(),
            rank: rows.value(&row.id, "rank").map_or(0, |rank| rank as u32),
            retrieved_at: rows
                .value(&row.id, "retrieved_at")
                .map_or(0, |at| at as i64),
        })
        .collect();
    WebCandidateSet::from_results(text_of("query_text"), text_of("argument_digest"), results)
}
