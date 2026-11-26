//! Executing foreign leaves through the optimizer: the `Op::ForeignScan` / `Op::Foreign`
//! arms of the executor land here.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use medulla_query_types::wire::ForeignSourceSpec;

use super::budget::{
    quota_refusal, unavailable, BUDGET_EXCEEDED, REPLICA_LAG_EXCEEDED, REQUIRES_KEYS,
    RESULT_INCOMPLETE, SNAPSHOT_UNAVAILABLE, SOURCE_QUOTA_EXCEEDED, SOURCE_WRITE_FORBIDDEN,
};
use super::capability::{
    in_matrix, require_in_matrix, FullFetch, KeyLookup, LimitPushdown, PageRequest, Paging,
    PushdownKind, RemoteRequest, SourceCapabilities, SourceRate,
};
use super::http::PAGE_SIZE;
use super::limiter;
use super::plan_evolution::{self, KeptPlan, ReplayInput, ReplayMeasure};
use super::remote::RemoteFetch;
use super::session::FederationSession;
use super::snapshot::{Consistency, SnapshotHandle};
use super::strategy::{choose_join, BatchSizer, JoinInputs, JoinStrategy};
use super::trace::{FetchStrategy, FragmentExecutionMode, FragmentTrace};
use super::{fuse_foreign, stats};
use crate::federation::ForeignSourceRegistry;
use medulla_catalog_types::metadata_acceleration::SnapshotSemantics;
use medulla_exec::rowset::RowSet;
use medulla_query_types::wire::Op;

/// `Op::ForeignScan { source, join }`: a source read (`join == false`, or an empty input) or
/// a foreign∩local join, optimized unless `MEDULLA_FEDERATION_OPT=0`.
pub(crate) fn foreign_scan(
    op: &Op,
    input: RowSet,
    source: &ForeignSourceSpec,
    join: bool,
    registry: Option<&ForeignSourceRegistry>,
    session: Option<&FederationSession>,
) -> Result<RowSet, String> {
    if let Some(raw) = super::enabled()
        .then(|| super::raw::resolve_raw(source, registry))
        .flatten()
    {
        return with_session(session, |session| {
            super::raw::raw_scan(op, &input, join, &raw, session)
        });
    }
    if let Some(search) = super::enabled()
        .then(|| super::remote::resolve_search(source, registry))
        .flatten()
    {
        return with_session(session, |session| {
            super::search::search_scan(op, &input, join, search.as_ref(), session)
        });
    }
    if let Some(web) = super::enabled()
        .then(|| super::web::resolve_web(source, registry))
        .flatten()
    {
        return with_session(session, |session| {
            super::web::web_scan(op, &input, join, &web, session)
        });
    }
    let remote = super::enabled()
        .then(|| super::remote::resolve(source, registry))
        .flatten();
    let Some(remote) = remote else {
        return Ok(fuse_foreign(
            input,
            super::foreign_source_rows(source, registry)?,
            join,
        ));
    };
    with_session(session, |session| {
        let mut fragment = Fragment::new(remote.as_ref(), session);
        fragment.bind_snapshot(source)?;
        fragment.kept = kept_for(session, remote.as_ref());
        if join && !input.is_empty() {
            let mut fetched = fragment.join(&input)?;
            fetched.replay = Some(replay_of(session, source, None, Some(&input)));
            let kept = input.intersect_keep_order(&fetched.rows.id_set());
            return Ok(fetched.record(session, kept));
        }
        let hint = session.limit_hint(op);
        let mut read = fragment.source(hint)?;
        read.replay = Some(replay_of(session, source, hint, None));
        Ok(read.finish(session))
    })
}

/// `Op::Foreign { name }` (UQL `FOREIGN "<name>"`): the named source's rows replace the
/// input, with a following `Limit` pushed when the source allows it.
pub(crate) fn foreign_named(
    op: &Op,
    name: &str,
    registry: Option<&ForeignSourceRegistry>,
    session: Option<&FederationSession>,
) -> Result<RowSet, String> {
    let registry =
        registry.ok_or_else(|| "FOREIGN requires a bound foreign-source registry".to_string())?;
    let remote = super::enabled()
        .then(|| super::remote::resolve_named(name, registry))
        .flatten();
    let Some(remote) = remote else {
        return registry.resolve(name);
    };
    with_session(session, |session| {
        let mut fragment = Fragment::new(remote.as_ref(), session);
        fragment.kept = kept_for(session, remote.as_ref());
        let hint = session.limit_hint(op);
        let mut read = fragment.source(hint)?;
        let named = ForeignSourceSpec::Named {
            name: name.to_string(),
        };
        read.replay = Some(replay_of(session, &named, hint, None));
        Ok(read.finish(session))
    })
}

/// The plan a plan-evolution run kept for the shape of `remote`, under the verified owner of
/// `session` (MEDULLA-FEDERATED-QUERY-R091.3). A standalone plan holds no owner and reads none.
fn kept_for(session: &FederationSession, remote: &dyn RemoteFetch) -> Option<KeptPlan> {
    let scope = session.cache_scope()?;
    plan_evolution::kept_plan(
        scope.owner_salt(),
        &plan_evolution::shape_digest_of(&remote.label()),
    )
}

/// What a replay of this fragment reads: the source, the limit hint and, for a join, the join's
/// input ids truncated to `max_bind_keys`. A standalone plan records nothing, so it builds none.
fn replay_of(
    session: &FederationSession,
    source: &ForeignSourceSpec,
    hint: Option<usize>,
    join_input: Option<&RowSet>,
) -> ReplayInput {
    if session.cache_scope().is_none() {
        return ReplayInput::default();
    }
    let max = session.budget().max_bind_keys;
    ReplayInput {
        source: source.clone(),
        hint,
        keys: join_input
            .map(|input| input.ids().into_iter().take(max).collect())
            .unwrap_or_default(),
    }
}

/// Run one recorded fragment again under `forced` (MEDULLA-FEDERATED-QUERY-R091.3): the kept
/// plan decides the fetch strategy and the limit, and the result is what the plan keeps. A
/// plan-evolution run replays the baseline and each candidate through this call. Rows received
/// stand in for bytes fetched: a source reports no byte count.
pub fn replay_fragment(
    remote: &dyn RemoteFetch,
    input: &ReplayInput,
    forced: &KeptPlan,
) -> Result<ReplayMeasure, String> {
    let session = FederationSession::from_env();
    let mut fragment = Fragment::new(remote, &session);
    fragment.kept = Some(forced.clone());
    let started = Instant::now();
    let (read, mut results) = if input.keys.is_empty() {
        let read = fragment.source(input.hint)?;
        let results = read.rows.ids();
        (read, results)
    } else {
        let keys = RowSet::from_rows(input.keys.iter().map(|key| (key.clone(), None)));
        let read = fragment.join(&keys)?;
        let results = keys.intersect_keep_order(&read.rows.id_set()).ids();
        (read, results)
    };
    // A source read with a hint keeps its first `hint` rows whichever way it was fetched.
    if let Some(hint) = input.hint.filter(|_| input.keys.is_empty()) {
        results.truncate(hint);
    }
    Ok(ReplayMeasure {
        latency_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        bytes_fetched: u64::try_from(read.trace.rows_fetched).unwrap_or(u64::MAX),
        results,
    })
}

/// Run `f` under the bound session, or a fresh server-budget session when none is bound.
fn with_session<R>(
    bound: Option<&FederationSession>,
    f: impl FnOnce(&FederationSession) -> R,
) -> R {
    match bound {
        Some(session) => f(session),
        None => f(&FederationSession::from_env()),
    }
}

/// Is `error` one of the optimizer's typed refusals (never retried or fallen back from)?
fn is_refusal(error: &str) -> bool {
    [
        BUDGET_EXCEEDED,
        REQUIRES_KEYS,
        RESULT_INCOMPLETE,
        SNAPSHOT_UNAVAILABLE,
        REPLICA_LAG_EXCEEDED,
        SOURCE_QUOTA_EXCEEDED,
        SOURCE_WRITE_FORBIDDEN,
    ]
    .iter()
    .any(|code| error.starts_with(code))
}

/// The rows a fragment read and its trace, before the residual decides what is kept.
pub(super) struct Read {
    pub(super) rows: RowSet,
    pub(super) trace: FragmentTrace,
    /// What a replay of this fragment reads, set by the foreign leaf that knows its source.
    replay: Option<ReplayInput>,
    started: Instant,
}

impl Read {
    /// A source read keeps every row it read.
    fn finish(mut self, session: &FederationSession) -> RowSet {
        let rows = std::mem::take(&mut self.rows);
        self.record(session, rows)
    }

    #[cfg(test)]
    pub(super) fn finish_for_test(self, session: &FederationSession) -> RowSet {
        self.finish(session)
    }

    /// Finish the trace with what the plan keeps, record it, and hand the kept rows on.
    pub fn record(mut self, session: &FederationSession, kept: RowSet) -> RowSet {
        self.trace.rows_kept = kept.len();
        self.trace.elapsed_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // MEDULLA-FEDERATED-QUERY-R091.3: a served query's fragment is one observation of its
        // shape, keyed by the verified owner. A standalone plan holds no owner and records nothing.
        if let Some(scope) = session.cache_scope() {
            plan_evolution::observe(
                scope.owner_salt(),
                &self.trace,
                self.replay.take().unwrap_or_default(),
            );
        }
        session.record(self.trace);
        kept
    }
}

/// One remote fragment of one query.
pub(super) struct Fragment<'r, 's> {
    remote: &'r dyn RemoteFetch,
    session: &'s FederationSession,
    caps: SourceCapabilities,
    /// The snapshot every fragment of this source reads, once bound.
    snapshot: Option<SnapshotSemantics>,
    consistency: Consistency,
    /// The plan kept for this fragment's shape, read before the fragment chooses
    /// (MEDULLA-FEDERATED-QUERY-R091.3).
    kept: Option<KeptPlan>,
}

impl<'r, 's> Fragment<'r, 's> {
    pub(super) fn new(remote: &'r dyn RemoteFetch, session: &'s FederationSession) -> Self {
        let mut caps = remote.capabilities();
        // The protocol's matrix row bounds the key lookup too: a row without `BatchedLookup`
        // is never sent a keyed predicate, whatever its source declared (R073.2).
        if !in_matrix(remote.protocol(), PushdownKind::BatchedLookup) {
            caps.key_lookup = KeyLookup::Unsupported;
        }
        Self {
            remote,
            session,
            caps,
            snapshot: None,
            consistency: Consistency::Snapshot,
            kept: None,
        }
    }

    /// Give this fragment its query's one snapshot of the source (MEDULLA-UNIFIED-DATA-PLANE-R038.3.3).
    /// Only a query with two or more fragments of `source` needs one. The first fragment opens it;
    /// later fragments adopt the held connection. A source without snapshots refuses with
    /// `SNAPSHOT_UNAVAILABLE` unless the caller accepted `consistency: per_fragment`. A refusal
    /// issues no source statement.
    pub(super) fn bind_snapshot(&mut self, source: &ForeignSourceSpec) -> Result<(), String> {
        if self.session.fragments_of(source) < 2 {
            return Ok(());
        }
        let fingerprint = self.remote.fingerprint();
        if let Some(handle) = self.session.snapshot_of(&fingerprint) {
            if let Some(conn) = handle.conn {
                self.remote.adopt_snapshot(conn);
            }
            self.snapshot = Some(handle.semantics);
            return Ok(());
        }
        let opened = if self.caps.snapshot_reads {
            self.remote.open_snapshot(&self.session.snapshot_bound())?
        } else {
            None
        };
        let Some(semantics) = opened else {
            return match self.session.consistency() {
                Consistency::PerFragment => {
                    self.consistency = Consistency::PerFragment;
                    Ok(())
                }
                Consistency::Snapshot => Err(format!(
                    "{SNAPSHOT_UNAVAILABLE}: the source cannot give two fragments of one query a single snapshot; request consistency=per_fragment to accept separate reads"
                )),
            };
        };
        self.session.hold_snapshot(
            fingerprint,
            SnapshotHandle {
                semantics: semantics.clone(),
                conn: self.remote.held_snapshot(),
            },
        );
        self.snapshot = Some(semantics);
        Ok(())
    }

    fn trace(&self, strategy: FetchStrategy) -> FragmentTrace {
        let mut trace =
            FragmentTrace::new(self.remote.label(), strategy, FragmentExecutionMode::Live);
        trace.snapshot = self.snapshot.clone();
        trace.consistency = self.consistency;
        trace.read_at_ms = super::cache::now_ms();
        trace
    }

    /// A source read: every row, or the first `hint` rows when the source takes a limit.
    pub(super) fn source(&self, hint: Option<usize>) -> Result<Read, String> {
        if self.caps.full_fetch == FullFetch::RequiresKeys {
            return Err(format!(
                "{REQUIRES_KEYS}: this source answers key lookups only; join it with a local candidate set"
            ));
        }
        let started = Instant::now();
        // The protocol's matrix row bounds the pushdown: a limit its row lacks is never sent.
        let wanted = hint
            .filter(|_| self.caps.limit == LimitPushdown::Native)
            .filter(|_| in_matrix(self.remote.protocol(), PushdownKind::LimitOrder));
        // A kept plan that did not push the limit sends none, even when the source could take it.
        let limit = wanted.filter(|_| self.kept.as_ref().is_none_or(|kept| kept.limit_pushed));
        let mut trace = self.trace(source_strategy(limit, self.caps.paging));
        trace.limit_pushed = limit;
        if wanted.is_some() && limit.is_none() {
            trace.kept_plan = self.kept.as_ref().map(|kept| kept.plan_digest.clone());
        }
        let request = RemoteRequest {
            limit,
            ..RemoteRequest::full()
        };
        let rows = match self.read_all(&request, &mut trace) {
            Ok(rows) if needs_refill(limit, self.caps.paging, &rows) => {
                trace.strategy = FetchStrategy::LimitRefill;
                self.read_full(&mut trace)?
            }
            Ok(rows) => self.observed(limit, rows),
            Err(e) if limit.is_none() || is_refusal(&e) => return Err(e),
            Err(_) => {
                trace.strategy = FetchStrategy::FallbackFullFetch;
                self.read_full(&mut trace)?
            }
        };
        Ok(Read {
            rows,
            trace,
            replay: None,
            started,
        })
    }

    /// Record an unlimited read's size in the source's statistics.
    fn observed(&self, limit: Option<usize>, rows: RowSet) -> RowSet {
        if limit.is_none() {
            stats::observe_full_fetch(&self.remote.fingerprint(), rows.len());
        }
        rows
    }

    /// Every row of the source (the naive request), recorded in the statistics.
    fn read_full(&self, trace: &mut FragmentTrace) -> Result<RowSet, String> {
        let rows = self.read_all(&RemoteRequest::full(), trace)?;
        Ok(self.observed(None, rows))
    }

    /// A foreign∩local join over `input`: bind join or full fetch, as the cost model says.
    pub(super) fn join(&self, input: &RowSet) -> Result<Read, String> {
        // A key-only source answers nothing without a keyed predicate its row must permit.
        if self.caps.full_fetch == FullFetch::RequiresKeys {
            require_in_matrix(self.remote.protocol(), PushdownKind::BatchedLookup)?;
        }
        let started = Instant::now();
        let keys = input.ids();
        let max_bind_keys = self.session.budget().max_bind_keys;
        let keys_expressible = keys.iter().all(|k| self.remote.key_expressible(k));
        let (chosen, estimate) = choose_join(&JoinInputs {
            caps: &self.caps,
            keys: keys.len(),
            keys_expressible,
            max_bind_keys,
            stats: stats::lookup(&self.remote.fingerprint()),
        });
        // The kept plan overrides the cost model's choice, never a refusal: a kept bind join
        // needs expressible keys within the bind bound, and a kept full fetch needs a source
        // that answers without keys.
        let bindable =
            self.caps.max_keys().is_some() && keys_expressible && keys.len() <= max_bind_keys;
        let kept_strategy = self.kept.as_ref().map(|kept| kept.strategy);
        let replacement = match (&chosen, kept_strategy) {
            (JoinStrategy::FullFetch, Some(FetchStrategy::BindJoin)) if bindable => {
                Some(JoinStrategy::BindJoin)
            }
            (JoinStrategy::BindJoin, Some(FetchStrategy::FullFetch))
                if self.caps.full_fetch != FullFetch::RequiresKeys =>
            {
                Some(JoinStrategy::FullFetch)
            }
            _ => None,
        };
        let overridden = replacement.is_some();
        let strategy = replacement.unwrap_or(chosen);
        let (rows, mut trace) = match strategy {
            JoinStrategy::Refuse(error) => return Err(error),
            JoinStrategy::BindJoin => self.bind_or_fallback(keys)?,
            JoinStrategy::FullFetch => {
                let mut trace = self.trace(FetchStrategy::FullFetch);
                (self.read_full(&mut trace)?, trace)
            }
        };
        trace.estimate = estimate;
        if overridden {
            trace.kept_plan = self.kept.as_ref().map(|kept| kept.plan_digest.clone());
        }
        Ok(Read {
            rows,
            trace,
            replay: None,
            started,
        })
    }

    /// Bind join; when key lookups keep failing, fall back to a full fetch (or fail for a
    /// key-only source).
    fn bind_or_fallback(&self, keys: Vec<String>) -> Result<(RowSet, FragmentTrace), String> {
        let mut trace = self.trace(FetchStrategy::BindJoin);
        trace.keys_pushed = keys.len();
        if let Some(rows) = self.bind(&keys, &mut trace)? {
            return Ok((rows, trace));
        }
        stats::observe_key_failure(&self.remote.fingerprint());
        if self.caps.full_fetch == FullFetch::RequiresKeys {
            return Err("federation: key lookups against a key-only source failed".to_string());
        }
        trace.strategy = FetchStrategy::FallbackFullFetch;
        let rows = self.read_full(&mut trace)?;
        Ok((rows, trace))
    }

    /// Ship `keys` in AIMD-sized batches. `Ok(None)` when the batches kept failing.
    fn bind(&self, keys: &[String], trace: &mut FragmentTrace) -> Result<Option<RowSet>, String> {
        if self.caps.paging == Paging::Single && keys.len() > 64 {
            if let Some(remote) = self.remote.parallel_safe() {
                if self.caps.rate.max_concurrent > 1 {
                    match self.bind_parallel(remote, keys, trace)? {
                        Some(rows) => return Ok(Some(rows)),
                        None => return self.bind_sequential(keys, trace),
                    }
                }
            }
        }
        self.bind_sequential(keys, trace)
    }

    /// Fixed windows are submitted in parallel. If a source rejects any window,
    /// retry the whole join with AIMD so the adaptive split and fallback semantics
    /// remain identical; successful speculative rows are never returned partially.
    fn bind_parallel(
        &self,
        remote: &(dyn RemoteFetch + Sync),
        keys: &[String],
        trace: &mut FragmentTrace,
    ) -> Result<Option<RowSet>, String> {
        let batch_size = self.caps.max_keys().unwrap_or(1).min(32);
        let mut out = Vec::new();
        let mut next = 0;
        while next < keys.len() {
            let mut requests = Vec::new();
            for _ in 0..self.caps.rate.max_concurrent {
                if next >= keys.len() {
                    break;
                }
                let window = &keys[next..keys.len().min(next + batch_size)];
                let fit = remote.fit_keys(window).clamp(1, window.len());
                requests.push(RemoteRequest::keys(window[..fit].to_vec()));
                next += fit;
            }
            let results = fetch_round(remote, self.session, self.caps, &requests);
            account(trace, &results);
            for attempt in results {
                match attempt.result {
                    Ok(rows) => {
                        out.extend(rows.rows().iter().map(|r| (r.id.clone(), r.score)));
                    }
                    Err(e) if is_refusal(&e) => return Err(e),
                    Err(_) => return Ok(None),
                }
            }
        }
        Ok(Some(RowSet::from_rows(out)))
    }

    fn bind_sequential(
        &self,
        keys: &[String],
        trace: &mut FragmentTrace,
    ) -> Result<Option<RowSet>, String> {
        let mut sizer = BatchSizer::new(self.caps.max_keys().unwrap_or(1));
        let mut out: Vec<(String, Option<f32>)> = Vec::new();
        let mut next = 0;
        while next < keys.len() {
            let window = &keys[next..keys.len().min(next + sizer.size())];
            let batch = &window[..self.remote.fit_keys(window).clamp(1, window.len())];
            let sent = Instant::now();
            match self.read_all(&RemoteRequest::keys(batch.to_vec()), trace) {
                Ok(rows) => {
                    sizer.success(sent.elapsed());
                    out.extend(rows.rows().iter().map(|r| (r.id.clone(), r.score)));
                    next += batch.len();
                }
                Err(e) if is_refusal(&e) => return Err(e),
                Err(_) if sizer.failure(batch.len()) => {}
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(RowSet::from_rows(out)))
    }

    /// Answer one request, page by page when the source pages.
    fn read_all(
        &self,
        request: &RemoteRequest,
        trace: &mut FragmentTrace,
    ) -> Result<RowSet, String> {
        if request.limit.is_some() {
            require_in_matrix(self.remote.protocol(), PushdownKind::LimitOrder)?;
        }
        match self.caps.paging {
            Paging::Single => self.round_trip(request, trace),
            Paging::Offset | Paging::Page => self.read_pages(request, trace),
        }
    }

    /// Pages until an empty page, or until `request.limit` distinct rows arrived. A source
    /// that never runs dry is stopped by the request budget (a typed refusal).
    fn read_pages(
        &self,
        request: &RemoteRequest,
        trace: &mut FragmentTrace,
    ) -> Result<RowSet, String> {
        let mut rows: Vec<(String, Option<f32>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut page = PageRequest {
            index: 0,
            offset: 0,
            size: PAGE_SIZE,
        };
        loop {
            let paged = RemoteRequest {
                page: Some(page),
                ..request.clone()
            };
            let got = self.round_trip(&paged, trace)?;
            if got.is_empty() {
                break;
            }
            page.index += 1;
            page.offset += got.len();
            for row in got.rows() {
                seen.insert(row.id.clone());
                rows.push((row.id.clone(), row.score));
            }
            if request.limit.is_some_and(|k| seen.len() >= k) {
                break;
            }
        }
        Ok(RowSet::from_rows(rows))
    }

    /// One budgeted round trip.
    fn round_trip(
        &self,
        request: &RemoteRequest,
        trace: &mut FragmentTrace,
    ) -> Result<RowSet, String> {
        let attempt = fetch_one(self.remote, self.session, self.caps, request);
        trace.requests += attempt.requests;
        trace.rows_fetched += attempt.rows;
        attempt.note_route(trace);
        attempt.result
    }
}

/// One charged and source-admitted network call. The permit is held only during fetch,
/// and the wait for it ends at the query's wall deadline.
struct Attempt {
    requests: u32,
    rows: usize,
    result: Result<RowSet, String>,
    /// The checkpoint of the stored copy that answered, when one did.
    copy: Option<String>,
    /// Why a stored copy did not answer (MEDULLA-UNIFIED-DATA-PLANE-R038.2.2).
    copy_refusal: Option<String>,
}

impl Attempt {
    fn refused(error: String) -> Self {
        Self {
            requests: 0,
            rows: 0,
            result: Err(error),
            copy: None,
            copy_refusal: None,
        }
    }

    /// Fold this attempt's route into the fragment trace: a copy-served attempt sets the
    /// fragment's mode to `copy` with the copy's watermark and row count.
    fn note_route(&self, trace: &mut FragmentTrace) {
        if self.copy_refusal.is_some() {
            trace.copy_refusal = self.copy_refusal.clone();
        }
        let Some(watermark) = &self.copy else { return };
        trace.mode = FragmentExecutionMode::Copy;
        trace.watermark = Some(watermark.clone());
        let served = self.result.as_ref().map_or(0, RowSet::len) as u64;
        trace.copy_rows = Some(trace.copy_rows.unwrap_or(0) + served);
    }
}

/// Send one round of requests concurrently; every attempt has finished when this returns.
fn fetch_round(
    remote: &(dyn RemoteFetch + Sync),
    session: &FederationSession,
    caps: SourceCapabilities,
    requests: &[RemoteRequest],
) -> Vec<Attempt> {
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(requests.len());
        for request in requests {
            handles.push(scope.spawn(move || fetch_one(remote, session, caps, request)));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("foreign request panicked"))
            .collect()
    })
}

/// Add every attempt of one parallel round to the trace. The whole round ran before any
/// result is read, so a failed batch does not hide the requests and rows of the others.
fn account(trace: &mut FragmentTrace, attempts: &[Attempt]) {
    trace.requests += attempts.iter().map(|a| a.requests).sum::<u32>();
    trace.rows_fetched += attempts.iter().map(|a| a.rows).sum::<usize>();
    for attempt in attempts {
        attempt.note_route(trace);
    }
}

fn fetch_one(
    remote: &dyn RemoteFetch,
    session: &FederationSession,
    caps: SourceCapabilities,
    request: &RemoteRequest,
) -> Attempt {
    let cache_scope = session.cache_scope();
    let cache_name = remote.identity().cache_name.as_deref();
    let label = remote.label();
    let fingerprint = remote.fingerprint();
    // MEDULLA-UNIFIED-DATA-PLANE-R031.2: this source's own bounds, narrowed by the query's.
    let quota = session.quota_for(remote.quota());
    if let (Some(scope), Some(name)) = (cache_scope.as_deref(), cache_name) {
        // MEDULLA-UNIFIED-DATA-PLANE-R038.2.2: a source registered with an acceleration policy
        // reads a stored copy only while the policy resolves `Copy` inside the row bound; any
        // other resolution, and a refused copy, runs live.
        let mut copy_refusal = None;
        let replica = scope
            .get(name, &fingerprint, request)
            .filter(|rows| match remote.acceleration() {
                None => true,
                Some(policy) => match super::accel::copy_route(
                    policy,
                    super::cache::now_ms(),
                    rows.len(),
                    session.copy_knobs(),
                ) {
                    Ok(()) => true,
                    Err(super::accel::LiveReason::Policy) => false,
                    Err(super::accel::LiveReason::Refused(text)) => {
                        copy_refusal = Some(text);
                        false
                    }
                },
            });
        if let Some(rows) = replica {
            let charged = session
                .with_meter(|meter| meter.check_wall())
                .and_then(|()| match quota {
                    Some(quota) => {
                        session.charge_source_rows(fingerprint, &label, rows.len(), quota)
                    }
                    None => Ok(()),
                })
                .and_then(|()| session.with_meter(|meter| meter.charge_rows(rows.len())));
            if let Err(error) = charged {
                return Attempt::refused(error);
            }
            return Attempt {
                requests: 0,
                rows: 0,
                result: Ok(rows),
                copy: scope.watermark_of(name).or_else(|| Some(String::new())),
                copy_refusal: None,
            };
        }
        return fetch_live(remote, session, caps, request, copy_refusal);
    }
    fetch_live(remote, session, caps, request, None)
}

/// The live leg of [`fetch_one`]: one charged, source-admitted network call.
fn fetch_live(
    remote: &dyn RemoteFetch,
    session: &FederationSession,
    caps: SourceCapabilities,
    request: &RemoteRequest,
    copy_refusal: Option<String>,
) -> Attempt {
    let cache_scope = session.cache_scope();
    let cache_name = remote.identity().cache_name.as_deref();
    let label = remote.label();
    let fingerprint = remote.fingerprint();
    let quota = session.quota_for(remote.quota());
    if let Err(e) = session.with_meter(|m| m.charge_request()) {
        return Attempt::refused(e);
    }
    let deadline = session.with_meter(|m| m.deadline());
    // The source's concurrency is the smaller of what its kind allows and its quota; waiting
    // for a permit ends at the query's deadline or one source wall, whichever is first, so a
    // source holding its own permits never holds another source's.
    let (rate, admit_by) = match quota {
        Some(quota) => (
            SourceRate::new(
                caps.rate
                    .max_concurrent
                    .min(usize::try_from(quota.max_concurrent).unwrap_or(usize::MAX)),
                caps.rate.requests_per_second,
            ),
            deadline.min(Instant::now() + Duration::from_millis(quota.max_wall_ms)),
        ),
        None => (caps.rate, deadline),
    };
    let Some(_permit) = limiter::acquire(fingerprint, rate, admit_by) else {
        return Attempt::refused(match quota {
            Some(quota) if Instant::now() < deadline => {
                quota_refusal(&label, "max_concurrent", quota.max_concurrent)
            }
            _ => session.with_meter(|m| m.wall_refusal()),
        });
    };
    let sent = Instant::now();
    let result = remote
        .fetch(request)
        .map_err(|error| unavailable(&label, error));
    let ms = u64::try_from(sent.elapsed().as_millis()).unwrap_or(u64::MAX);
    stats::observe_request(&fingerprint, ms);
    let rows = result.as_ref().map_or(0, RowSet::len);
    let result = match quota {
        Some(quota) if ms >= quota.max_wall_ms => {
            Err(quota_refusal(&label, "max_wall_ms", quota.max_wall_ms))
        }
        _ => result,
    };
    let result = result.and_then(|rows| {
        if let Some(quota) = quota {
            session.charge_source_rows(fingerprint, &label, rows.len(), quota)?;
        }
        session.with_meter(|m| m.charge_rows(rows.len()))?;
        if let (Some(scope), Some(name)) = (cache_scope.as_deref(), cache_name) {
            scope.insert(name, &fingerprint, request, &rows);
        }
        Ok(rows)
    });
    Attempt {
        requests: 1,
        rows,
        result,
        copy: None,
        copy_refusal,
    }
}

/// The trace label of a source read.
fn source_strategy(limit: Option<usize>, paging: Paging) -> FetchStrategy {
    match (limit, paging) {
        (_, Paging::Offset | Paging::Page) => FetchStrategy::Paged,
        (Some(_), Paging::Single) => FetchStrategy::LimitPushdown,
        (None, Paging::Single) => FetchStrategy::FullFetch,
    }
}

/// A single-request limited read that de-duplicated below `k` rows may have cut rows the
/// naive read keeps; it is refilled from a full read.
fn needs_refill(limit: Option<usize>, paging: Paging, rows: &RowSet) -> bool {
    paging == Paging::Single && limit.is_some_and(|k| rows.len() < k)
}

#[cfg(test)]
mod cache_tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::federation_opt::cache::{now_ms, FragmentCacheScope, SourceWatermark};
    use crate::federation_opt::remote::Identity;

    struct Counted {
        identity: Identity,
        calls: AtomicUsize,
    }

    impl RemoteFetch for Counted {
        fn capabilities(&self) -> SourceCapabilities {
            SourceCapabilities::fetch_only()
        }

        fn identity(&self) -> &Identity {
            &self.identity
        }

        fn fetch(&self, _request: &RemoteRequest) -> Result<RowSet, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(RowSet::from_rows(vec![("row".into(), None)]))
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R049
    #[test]
    fn served_fragment_reuses_only_fresh_named_owner_scope() {
        let remote = Counted {
            identity: Identity {
                label: "counted:crm".into(),
                fingerprint: [19; 32],
                cache_name: Some("crm".into()),
            },
            calls: AtomicUsize::new(0),
        };
        let scope = |owner: &str, watermark: &str, expiry| {
            Arc::new(FragmentCacheScope::new(
                owner.into(),
                HashMap::from([(
                    "crm".into(),
                    SourceWatermark {
                        watermark: watermark.into(),
                        valid_through_ms: expiry,
                        entry_ttl_ms: None,
                    },
                )]),
            ))
        };
        let session = FederationSession::new(medulla_exec::budget::FederationBudget::default());
        session.set_cache_scope(Some(scope("owner-one", "lsn-1", now_ms() + 30_000)));
        let request = RemoteRequest::keys(vec!["needle".into()]);
        let caps = remote.capabilities();
        let first = fetch_one(&remote, &session, caps, &request);
        assert_eq!(first.requests, 1);
        assert!(first.result.is_ok());
        let hit = fetch_one(&remote, &session, caps, &request);
        assert_eq!(hit.requests, 0);
        assert_eq!(remote.calls.load(Ordering::SeqCst), 1);

        session.set_cache_scope(Some(scope("owner-two", "lsn-1", now_ms() + 30_000)));
        assert_eq!(fetch_one(&remote, &session, caps, &request).requests, 1);
        session.set_cache_scope(Some(scope("owner-one", "lsn-2", now_ms() + 30_000)));
        assert_eq!(fetch_one(&remote, &session, caps, &request).requests, 1);
        session.set_cache_scope(Some(scope("owner-one", "lsn-1", now_ms() - 1)));
        assert_eq!(fetch_one(&remote, &session, caps, &request).requests, 1);
        session.set_cache_scope(None);
        assert_eq!(fetch_one(&remote, &session, caps, &request).requests, 1);
        assert_eq!(remote.calls.load(Ordering::SeqCst), 5);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::federation_opt::capability::KeyLookup;
    use crate::federation_opt::remote::Identity;

    /// A source over a fixed catalog that answers key lookups and full reads and counts them.
    struct Keyed {
        identity: Identity,
        caps: SourceCapabilities,
        expressible: bool,
        catalog: Vec<String>,
        keyed_calls: AtomicUsize,
        full_calls: AtomicUsize,
    }

    impl Keyed {
        fn new(name: &str, caps: SourceCapabilities, expressible: bool) -> Self {
            Self {
                identity: Identity {
                    label: format!("registered:{name}#00000000"),
                    fingerprint: stats::fingerprint(name.as_bytes()),
                    cache_name: None,
                },
                caps,
                expressible,
                catalog: (0..20).map(|i| format!("x{i}")).collect(),
                keyed_calls: AtomicUsize::new(0),
                full_calls: AtomicUsize::new(0),
            }
        }
    }

    impl RemoteFetch for Keyed {
        fn capabilities(&self) -> SourceCapabilities {
            self.caps
        }

        fn identity(&self) -> &Identity {
            &self.identity
        }

        fn key_expressible(&self, _key: &str) -> bool {
            self.expressible
        }

        fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
            let rows: Vec<(String, Option<f32>)> = if request.keys.is_empty() {
                self.full_calls.fetch_add(1, Ordering::SeqCst);
                self.catalog.iter().map(|id| (id.clone(), None)).collect()
            } else {
                self.keyed_calls.fetch_add(1, Ordering::SeqCst);
                self.catalog
                    .iter()
                    .filter(|id| request.keys.contains(id))
                    .map(|id| (id.clone(), None))
                    .collect()
            };
            Ok(RowSet::from_rows(rows))
        }
    }

    fn kept(strategy: FetchStrategy) -> KeptPlan {
        KeptPlan {
            plan_digest: format!("kept-{strategy:?}"),
            strategy,
            limit_pushed: false,
            receipt_job_id: "job-1".into(),
            evidence: Default::default(),
        }
    }

    fn input() -> RowSet {
        RowSet::from_ids((0..5).map(|i| format!("x{i}")))
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.3
    #[test]
    fn kept_plan_overrides_join_choice_but_not_refusal() {
        let session = FederationSession::from_env();
        let caps = SourceCapabilities::single_full_fetch(
            KeyLookup::Batched { max_keys: 100 },
            LimitPushdown::Native,
        );

        // Unkept, the cost model chooses a bind join for an unobserved source.
        let source = Keyed::new("kept-join-a", caps, true);
        let read = Fragment::new(&source, &session).join(&input()).unwrap();
        assert_eq!(read.trace.strategy, FetchStrategy::BindJoin);
        assert_eq!(read.trace.kept_plan, None);

        // A kept full fetch replaces it, names its digest in the trace and returns the same rows.
        let source = Keyed::new("kept-join-b", caps, true);
        let mut fragment = Fragment::new(&source, &session);
        fragment.kept = Some(kept(FetchStrategy::FullFetch));
        let read = fragment.join(&input()).unwrap();
        assert_eq!(read.trace.strategy, FetchStrategy::FullFetch);
        assert_eq!(read.trace.kept_plan.as_deref(), Some("kept-FullFetch"));
        assert_eq!(source.keyed_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            input().intersect_keep_order(&read.rows.id_set()),
            input(),
            "the kept plan returns the rows the cost model's plan returns"
        );

        // A kept full fetch never replaces the only plan a key-only source has.
        let mut key_only = caps;
        key_only.full_fetch = FullFetch::RequiresKeys;
        let source = Keyed::new("kept-join-c", key_only, true);
        let mut fragment = Fragment::new(&source, &session);
        fragment.kept = Some(kept(FetchStrategy::FullFetch));
        let read = fragment.join(&input()).unwrap();
        assert_eq!(read.trace.strategy, FetchStrategy::BindJoin);
        assert_eq!(read.trace.kept_plan, None);

        // A kept bind join does not turn a refusal into a read: the keys cannot be expressed.
        let source = Keyed::new("kept-join-d", key_only, false);
        let mut fragment = Fragment::new(&source, &session);
        fragment.kept = Some(kept(FetchStrategy::BindJoin));
        let error = fragment.join(&input()).err().expect("the refusal is final");
        assert!(error.starts_with(REQUIRES_KEYS), "{error}");
        assert_eq!(source.keyed_calls.load(Ordering::SeqCst), 0);
        assert_eq!(source.full_calls.load(Ordering::SeqCst), 0);

        // A kept plan that did not push the limit sends none to a source that could take it.
        let source = Keyed::new("kept-limit", caps, true);
        let read = Fragment::new(&source, &session).source(Some(3)).unwrap();
        assert_eq!(read.trace.limit_pushed, Some(3));
        let mut fragment = Fragment::new(&source, &session);
        fragment.kept = Some(kept(FetchStrategy::FullFetch));
        let read = fragment.source(Some(3)).unwrap();
        assert_eq!(read.trace.limit_pushed, None);
        assert_eq!(read.trace.kept_plan.as_deref(), Some("kept-FullFetch"));
    }

    /// A [`Keyed`] source that speaks `protocol`.
    struct OnProtocol<'a> {
        inner: &'a Keyed,
        protocol: crate::federation_opt::capability::ConnectionProtocol,
    }

    impl RemoteFetch for OnProtocol<'_> {
        fn capabilities(&self) -> SourceCapabilities {
            self.inner.capabilities()
        }
        fn identity(&self) -> &Identity {
            self.inner.identity()
        }
        fn protocol(&self) -> Option<crate::federation_opt::capability::ConnectionProtocol> {
            Some(self.protocol)
        }
        fn key_expressible(&self, key: &str) -> bool {
            self.inner.key_expressible(key)
        }
        fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
            self.inner.fetch(request)
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R073.2
    #[test]
    fn the_protocol_row_bounds_the_bind_join() {
        use crate::federation_opt::capability::{ConnectionProtocol, PUSHDOWN_NOT_IN_MATRIX};
        let session = FederationSession::from_env();
        let caps = SourceCapabilities::single_full_fetch(
            KeyLookup::Batched { max_keys: 100 },
            LimitPushdown::Native,
        );
        // A SQL row carries the batched lookup: the bind join runs.
        let inner = Keyed::new("matrix-sql", caps, true);
        let sql = OnProtocol {
            inner: &inner,
            protocol: ConnectionProtocol::Sql,
        };
        let read = Fragment::new(&sql, &session).join(&input()).unwrap();
        assert_eq!(read.trace.strategy, FetchStrategy::BindJoin);
        assert!(inner.keyed_calls.load(Ordering::SeqCst) > 0);

        // A stream row does not: the source declared a key lookup, the row says no, and the
        // join falls back to one full read intersected locally.
        let inner = Keyed::new("matrix-stream", caps, true);
        let stream = OnProtocol {
            inner: &inner,
            protocol: ConnectionProtocol::EventStream,
        };
        let read = Fragment::new(&stream, &session).join(&input()).unwrap();
        assert_eq!(read.trace.strategy, FetchStrategy::FullFetch);
        assert_eq!(inner.keyed_calls.load(Ordering::SeqCst), 0);

        // A key-only source on a row without the lookup is refused with the typed code.
        let mut key_only = caps;
        key_only.full_fetch = FullFetch::RequiresKeys;
        let inner = Keyed::new("matrix-key-only", key_only, true);
        let stream = OnProtocol {
            inner: &inner,
            protocol: ConnectionProtocol::EventStream,
        };
        let refused = Fragment::new(&stream, &session)
            .join(&input())
            .err()
            .expect("the row forbids the keyed predicate");
        assert!(refused.starts_with(PUSHDOWN_NOT_IN_MATRIX), "{refused}");
        assert_eq!(inner.keyed_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inner.full_calls.load(Ordering::SeqCst), 0);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.3
    #[test]
    fn replay_runs_the_forced_plan_and_keeps_the_same_results() {
        let caps = SourceCapabilities::single_full_fetch(
            KeyLookup::Batched { max_keys: 100 },
            LimitPushdown::Native,
        );
        let source = Keyed::new("replay", caps, true);
        let spec = ForeignSourceSpec::Named {
            name: "replay".into(),
        };
        let join = ReplayInput {
            source: spec.clone(),
            hint: None,
            keys: (0..5).map(|i| format!("x{i}")).collect(),
        };
        let bind = replay_fragment(&source, &join, &kept(FetchStrategy::BindJoin)).unwrap();
        let full = replay_fragment(&source, &join, &kept(FetchStrategy::FullFetch)).unwrap();
        let mut left = bind.results.clone();
        let mut right = full.results.clone();
        left.sort();
        right.sort();
        assert_eq!(left, right);
        assert!(bind.bytes_fetched < full.bytes_fetched);

        // A source read with a hint keeps its first rows whether or not the limit was pushed.
        let read = ReplayInput {
            source: spec,
            hint: Some(3),
            keys: Vec::new(),
        };
        let mut pushed = kept(FetchStrategy::LimitPushdown);
        pushed.limit_pushed = true;
        let limited = replay_fragment(&source, &read, &pushed).unwrap();
        let unlimited = replay_fragment(&source, &read, &kept(FetchStrategy::FullFetch)).unwrap();
        assert_eq!(limited.results, unlimited.results);
        assert_eq!(limited.results.len(), 3);
    }
}
