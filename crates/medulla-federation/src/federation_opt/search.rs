//! Relevance search pushed into a search-capable source (MEDULLA-UNIFIED-DATA-PLANE-R038.5.2).
//!
//! The planner asks the source for `min(requested_k × 2, max_k)` results per request so the
//! exact local residual filter has room to drop some. The residual always runs. While fewer
//! than `k` rows survive and the index has more, the planner pages on; it stops at `k` or at
//! the shortfall the index leaves, which the outcome reports as data.

use sha2::{Digest, Sha256};

use super::budget::{EMBEDDING_SPACE_MISMATCH, REMOTE_SCORE_MISSING, RESULT_INCOMPLETE};
use super::capability::{PageRequest, RemoteRequest, SearchPush};
use super::remote::Identity;
use super::session::FederationSession;
use super::trace::{FetchStrategy, FragmentExecutionMode, FragmentTrace, SearchReceipt};
use medulla_types::search_source::{ScoreKind, SearchIndexCapability};

/// One result of a pushed search: the document id, the remote score (absent when the source
/// returned none; the engine never invents one) and the attributes the local residual filter
/// reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub score: Option<f32>,
    pub attributes: Vec<(String, String)>,
}

/// A source that answers a [`SearchPush`] in its own request. A request carries the push and
/// the page (`offset`, `size`) of results to return.
pub trait SearchRemote {
    fn identity(&self) -> &Identity;
    fn capability(&self) -> SearchIndexCapability;
    /// The embedding space the source's vectors live in; `None` when it has no vector index.
    fn vector_space(&self) -> Option<String> {
        None
    }
    fn fetch(&self, request: &RemoteRequest) -> Result<Vec<SearchHit>, String>;
}

/// How many rows the index could not supply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortfall {
    pub requested: usize,
    pub returned: usize,
}

/// The rows a pushed search kept, and the shortfall when the index ran dry first.
#[derive(Debug)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub shortfall: Option<Shortfall>,
    /// One `REMOTE_SCORE_MISSING: <id>` per row dropped for lacking a score.
    pub refused_rows: Vec<String>,
    /// `FEDERATION_RESULT_INCOMPLETE: ...` when a request after the first was refused; the
    /// rows already fetched are kept.
    pub incomplete: Option<String>,
}

/// The `k` pushed for a request of `requested` results: twice as many, capped at the
/// operation's `max_k`, so the residual filter has room.
pub fn pushed_k(requested: u32, max_k: u32) -> u32 {
    requested.saturating_mul(2).min(max_k).max(1)
}

/// SHA-256 over the canonical pushed request, as 64 lowercase hex characters.
pub fn query_digest(push: &SearchPush) -> String {
    let canonical = rmp_serde::to_vec_named(push).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

fn request_for(push: &SearchPush, index: usize, offset: usize) -> RemoteRequest {
    RemoteRequest {
        search: Some(push.clone()),
        page: Some(PageRequest {
            index,
            offset,
            size: push.k as usize,
        }),
        ..RemoteRequest::default()
    }
}

/// Run `wanted` against `remote`: push `min(2k, max_k)`, keep the hits `residual` accepts and
/// page on until `wanted.k` rows survive or the index has no more. Records one fragment trace
/// carrying the search receipt.
pub fn fetch_top_k(
    session: &FederationSession,
    remote: &dyn SearchRemote,
    wanted: &SearchPush,
    residual: &dyn Fn(&SearchHit) -> bool,
) -> Result<SearchOutcome, String> {
    let capability = remote.capability();
    if wanted.vector_space.is_some() && wanted.vector_space != remote.vector_space() {
        return Err(format!(
            "{EMBEDDING_SPACE_MISMATCH}: the query vector is not in the embedding space of {}",
            remote.identity().label
        ));
    }
    let push = SearchPush {
        k: pushed_k(wanted.k, capability.max_k),
        ..wanted.clone()
    };
    let (want, page) = (wanted.k as usize, push.k as usize);
    let mut kept: Vec<SearchHit> = Vec::new();
    let (mut requests, mut returned) = (0u32, 0usize);
    let mut refused_rows: Vec<String> = Vec::new();
    let mut incomplete = None;
    let scored = capability.score_kind != ScoreKind::Opaque;
    while kept.len() < want && want > 0 {
        let step = session
            .with_meter(|meter| meter.charge_request())
            .and_then(|()| remote.fetch(&request_for(&push, requests as usize, returned)))
            .and_then(|batch| {
                session.with_meter(|meter| meter.charge_rows(batch.len()))?;
                Ok(batch)
            });
        let batch = match step {
            Ok(batch) => batch,
            // A refusal after the first request keeps the rows already fetched.
            Err(refusal) if requests > 0 => {
                incomplete = Some(format!("{RESULT_INCOMPLETE}: {refusal}"));
                break;
            }
            Err(refusal) => return Err(refusal),
        };
        requests += 1;
        returned += batch.len();
        let exhausted = batch.len() < page;
        for hit in batch {
            if scored && hit.score.is_none() {
                refused_rows.push(format!("{REMOTE_SCORE_MISSING}: {}", hit.id));
            } else if residual(&hit) {
                kept.push(hit);
            }
        }
        if exhausted {
            break;
        }
    }
    kept.truncate(want);
    let receipt = SearchReceipt {
        query_digest: query_digest(&push),
        k: push.k,
        score_kind: capability.score_kind,
        returned,
    };
    record(session, remote, (requests, returned, kept.len()), receipt);
    let shortfall = (kept.len() < want).then_some(Shortfall {
        requested: want,
        returned: kept.len(),
    });
    Ok(SearchOutcome {
        hits: kept,
        shortfall,
        refused_rows,
        incomplete,
    })
}

fn record(
    session: &FederationSession,
    remote: &dyn SearchRemote,
    (requests, fetched, kept): (u32, usize, usize),
    receipt: SearchReceipt,
) {
    let mut trace = FragmentTrace::new(
        remote.identity().label.clone(),
        FetchStrategy::Paged,
        FragmentExecutionMode::Live,
    );
    trace.requests = requests;
    trace.rows_fetched = fetched;
    trace.rows_kept = kept;
    trace.search = Some(receipt);
    session.record(trace);
}
