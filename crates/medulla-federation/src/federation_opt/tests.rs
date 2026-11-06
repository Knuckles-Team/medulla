//! EH-563 phase-1 proofs. Oracles: (E) the optimized plan returns exactly the rows of the
//! naive full fetch; (B) what crossed the network — requests and items the mock source
//! served; (S) typed refusals. The HTTP proofs drive the real executor and the real
//! `HttpJsonSource` against a recording in-test API server (loopback, exact allow-list).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla_types::wire::{ForeignSourceSpec, HttpFieldMap};

use super::capability::{
    connection_protocol_row, ConnectionProtocol, ForeignSourceCapability, KeyLookup, LimitPushdown,
    PushdownKind, RemoteRequest, SourceCapabilities, SourceCostModel, SourceRate,
    CONNECTION_PROTOCOL_MATRIX,
};
use super::remote::{Identity, RemoteFetch};
use super::run::Fragment;
use super::stats::fingerprint;
use super::{FederationSession, FetchStrategy, BUDGET_EXCEEDED, REQUIRES_KEYS};
use crate::test_guard::MockHttpAllowGuard;
use medulla_exec::algebra::Op;
use medulla_exec::algebra::Plan;
use medulla_exec::budget::FederationBudget;
use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::rowset::RowSet;

// ── a recording JSON API ─────────────────────────────────────────────────────────

/// What the mock API has served.
#[derive(Default)]
struct Served {
    requests: AtomicUsize,
    items: AtomicUsize,
    queries: Mutex<Vec<String>>,
}

/// A loopback JSON API over `catalog`: `ids=` selects by id (comma list), `offset=`/`page=`
/// + `limit=` page it, `limit=` alone truncates; page size is capped at `cap`.
struct MockApi {
    base: String,
    served: Arc<Served>,
}

fn param<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = if bytes[i] == b'%' {
            value
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        } else {
            None
        };
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The items one request selects.
fn select(catalog: &[String], query: &str, cap: usize) -> Vec<String> {
    if let Some(ids) = param(query, "ids") {
        let wanted: Vec<String> = ids.split(',').map(decode).collect();
        return catalog
            .iter()
            .filter(|c| wanted.contains(c))
            .cloned()
            .collect();
    }
    let limit = param(query, "limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(cap)
        .min(cap);
    let offset = match (param(query, "offset"), param(query, "page")) {
        (Some(o), _) => o.parse().unwrap_or(0),
        (None, Some(p)) => p.parse::<usize>().unwrap_or(1).saturating_sub(1) * limit,
        (None, None) => 0,
    };
    catalog.iter().skip(offset).take(limit).cloned().collect()
}

impl MockApi {
    fn spawn(catalog: Vec<String>, cap: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock api");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(Served::default());
        let record = served.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(64) {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or("/");
                let query = target.split_once('?').map_or("", |(_, q)| q).to_string();
                let items = select(&catalog, &query, cap);
                record.requests.fetch_add(1, Ordering::SeqCst);
                record.items.fetch_add(items.len(), Ordering::SeqCst);
                record.queries.lock().unwrap().push(query);
                let body = serde_json::json!({
                    "data": items.iter().map(|id| serde_json::json!({"ref": id})).collect::<Vec<_>>()
                })
                .to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Self { base, served }
    }

    fn spec(&self, query: &str) -> ForeignSourceSpec {
        ForeignSourceSpec::HttpJson {
            url: format!("{}/items{query}", self.base),
            json_path: "data".into(),
            field_map: HttpFieldMap {
                id: "ref".into(),
                score: None,
                columns: Default::default(),
            },
        }
    }

    fn requests(&self) -> usize {
        self.served.requests.load(Ordering::SeqCst)
    }

    fn items(&self) -> usize {
        self.served.items.load(Ordering::SeqCst)
    }
}

/// `n` catalog ids; the fixture's `d2` and `d4` are among them.
fn catalog(n: usize) -> Vec<String> {
    let mut ids = vec!["d2".to_string(), "d4".to_string()];
    ids.extend((0..n.saturating_sub(2)).map(|i| format!("x{i}")));
    ids
}

fn ids(rows: &RowSet) -> Vec<String> {
    rows.ids()
}

fn execute_federated(
    fx: &medulla_exec::fixture::Fixture,
    session: &FederationSession,
    plan: Plan,
) -> Result<RowSet, String> {
    let federated = crate::federation_opt::FederatedExec::with_session(session);
    let ctx = PlanCtx::new(&fx.view, &fx.semantic).with_foreign_executor(&federated);
    execute(&plan, &ctx)
}

fn source_plan(spec: ForeignSourceSpec, limit: Option<usize>) -> Plan {
    let mut ops = vec![Op::ForeignScan {
        source: Box::new(spec),
        join: false,
    }];
    if let Some(k) = limit {
        ops.push(Op::Limit { k });
    }
    Plan::new(ops)
}

// spec: MEDULLA-FEDERATED-QUERY-R041
#[test]
fn protocol_matrix_bounds_pushdown() {
    let capability = ForeignSourceCapability::new(
        SourceCapabilities::single_full_fetch(KeyLookup::Unsupported, LimitPushdown::Native),
        SourceCostModel::new(100, 1),
        true,
        true,
    );

    // Every declared protocol must have exactly one planner row. This catches a
    // newly declared transport that silently falls through to unrestricted pushdown.
    assert_eq!(
        CONNECTION_PROTOCOL_MATRIX.len(),
        ConnectionProtocol::ALL.len()
    );
    for protocol in ConnectionProtocol::ALL {
        let row = connection_protocol_row(*protocol)
            .unwrap_or_else(|| panic!("declared protocol {protocol:?} has no matrix row"));
        assert_eq!(row.protocol, *protocol);
        assert!(!row.contract_term.is_empty());
    }

    // Streaming rows are deliberately narrow: a time range is the only
    // pushdown they permit, and a source capability cannot widen that row.
    let stream = connection_protocol_row(ConnectionProtocol::EventStream).unwrap();
    assert_eq!(stream.pushdown, &[PushdownKind::TimeRange]);
    assert!(stream.allows(PushdownKind::TimeRange));
    assert!(!stream.allows(PushdownKind::Filter));
    assert!(!capability.proves_sound_for(ConnectionProtocol::EventStream, PushdownKind::Filter));

    // The same source capability is usable for a declared REST filter but not
    // for a webhook, whose row has no planner pushdown at all.
    assert!(capability.proves_sound_for(ConnectionProtocol::RestOpenApi, PushdownKind::Filter));
    let webhook = connection_protocol_row(ConnectionProtocol::Webhook).unwrap();
    assert!(webhook.pushdown.is_empty());
    assert!(!capability.proves_sound_for(ConnectionProtocol::Webhook, PushdownKind::Filter));
}

// ── (E)+(B): HTTP through the executor ──────────────────────────────────────────

// spec: MEDULLA-FEDERATED-QUERY-R041
#[test]
fn a_foreign_join_ships_local_ids_instead_of_fetching_the_source() {
    let fx = medulla_exec::fixture::build();
    let api = MockApi::spawn(catalog(1000), 1000);
    let _allow = MockHttpAllowGuard::new(&api.base);
    let spec = api.spec("?ids={keys}");
    let local = execute(
        &Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]),
        &PlanCtx::new(&fx.view, &fx.semantic),
    )
    .unwrap();
    let expected: Vec<String> = ids(&local)
        .into_iter()
        .filter(|id| id == "d2" || id == "d4")
        .collect();

    let session = FederationSession::from_env();
    let plan = Plan::new(vec![
        Op::Scan {
            label: "Doc".into(),
        },
        Op::ForeignScan {
            source: Box::new(spec),
            join: true,
        },
    ]);
    let joined = execute_federated(&fx, &session, plan).unwrap();

    assert_eq!(
        ids(&joined),
        expected,
        "(E) the bind join equals local ∩ catalog"
    );
    assert_eq!(api.requests(), 1, "(B) one batch carried every local id");
    assert_eq!(
        api.items(),
        2,
        "(B) two items moved, not the 1000-item catalog"
    );
    let trace = session.trace();
    assert_eq!(trace[0].strategy, FetchStrategy::BindJoin);
    assert_eq!(trace[0].keys_pushed, local.len());
}

// spec: MEDULLA-FEDERATED-QUERY-R071
#[test]
fn a_paged_source_is_read_to_its_end_not_truncated_to_page_one() {
    let api = MockApi::spawn(catalog(250), 40);
    let _allow = MockHttpAllowGuard::new(&api.base);
    let spec = api.spec("?offset={offset}&limit={limit}");
    let naive = super::foreign_source_rows(&spec, None).unwrap();
    assert_eq!(naive.len(), 40, "the naive single GET sees page one only");

    let fx = medulla_exec::fixture::build();
    let session = FederationSession::from_env();
    let plan = source_plan(spec, None);
    let rows = execute_federated(&fx, &session, plan).unwrap();
    assert_eq!(ids(&rows), catalog(250), "(E) every page, in source order");
    assert_eq!(session.trace()[0].strategy, FetchStrategy::Paged);
}

#[test]
fn a_following_limit_is_pushed_into_the_source() {
    let api = MockApi::spawn(catalog(500), 1000);
    let _allow = MockHttpAllowGuard::new(&api.base);
    let spec = api.spec("?limit={limit}");
    let fx = medulla_exec::fixture::build();
    let session = FederationSession::from_env();
    let plan = source_plan(spec, Some(5));
    let rows = execute_federated(&fx, &session, plan).unwrap();
    assert_eq!(
        ids(&rows),
        catalog(500)[..5].to_vec(),
        "(E) the first five, as the naive plan"
    );
    assert_eq!(api.items(), 5, "(B) five items moved");
    assert!(api
        .served
        .queries
        .lock()
        .unwrap()
        .iter()
        .all(|q| q == "limit=5"));
    assert_eq!(session.trace()[0].limit_pushed, Some(5));
}

// spec: MEDULLA-FEDERATED-QUERY-R041
// spec: MEDULLA-FEDERATED-QUERY-R071
#[test]
fn a_key_only_source_refuses_a_scan_without_keys() {
    let api = MockApi::spawn(catalog(10), 10);
    let _allow = MockHttpAllowGuard::new(&api.base);
    let fx = medulla_exec::fixture::build();
    let federated = crate::federation_opt::FederatedExec::default();
    let ctx = PlanCtx::new(&fx.view, &fx.semantic).with_foreign_executor(&federated);
    let plan = Plan::new(vec![Op::ForeignScan {
        source: Box::new(api.spec("?ids={keys}")),
        join: false,
    }]);
    let err = execute(&plan, &ctx).unwrap_err();
    assert!(err.starts_with(REQUIRES_KEYS), "{err}");
    assert_eq!(api.requests(), 0, "(S) refused before any request");
}

// spec: MEDULLA-FEDERATED-QUERY-R041
#[test]
fn the_request_budget_refuses_instead_of_returning_partial_rows() {
    let api = MockApi::spawn(catalog(250), 40);
    let _allow = MockHttpAllowGuard::new(&api.base);
    let fx = medulla_exec::fixture::build();
    let session = FederationSession::new(FederationBudget {
        max_requests: 2,
        ..FederationBudget::default()
    });
    let plan = source_plan(api.spec("?page={page}&limit={limit}"), None);
    let err = execute_federated(&fx, &session, plan).unwrap_err();
    assert!(
        err.starts_with(&format!("{BUDGET_EXCEEDED}:requests")),
        "{err}"
    );
    assert_eq!(api.requests(), 2);
}

// spec: MEDULLA-FEDERATED-QUERY-R071
#[test]
fn placeholders_outside_the_query_string_never_template_the_destination() {
    let fx = medulla_exec::fixture::build();
    let federated = crate::federation_opt::FederatedExec::default();
    let ctx = PlanCtx::new(&fx.view, &fx.semantic).with_foreign_executor(&federated);
    let spec = ForeignSourceSpec::HttpJson {
        url: "http://{keys}.example.invalid/items".into(),
        json_path: String::new(),
        field_map: HttpFieldMap {
            id: "ref".into(),
            score: None,
            columns: Default::default(),
        },
    };
    let err = execute(
        &Plan::new(vec![Op::ForeignScan {
            source: Box::new(spec),
            join: false,
        }]),
        &ctx,
    )
    .unwrap_err();
    assert!(err.contains("federation"), "{err}");
}

// ── (E)+(B): the bind-join engine against scripted sources ──────────────────────

/// A key-lookup source over `catalog` that fails any batch larger than `max_ok` keys (or
/// every key batch when `max_ok == 0`) and counts its round trips.
struct Scripted {
    catalog: Vec<String>,
    max_ok: usize,
    calls: AtomicUsize,
    identity: Identity,
}

impl Scripted {
    fn new(tag: &str, catalog: Vec<String>, max_ok: usize) -> Self {
        Self {
            catalog,
            max_ok,
            calls: AtomicUsize::new(0),
            identity: Identity {
                label: "scripted".into(),
                fingerprint: fingerprint(tag.as_bytes()),
                cache_name: None,
            },
        }
    }
}

impl RemoteFetch for Scripted {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::single_full_fetch(
            KeyLookup::Batched { max_keys: 1000 },
            LimitPushdown::Unsupported,
        )
    }
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !request.keys.is_empty() && request.keys.len() > self.max_ok {
            return Err("scripted: batch too large".into());
        }
        let keep = |id: &String| request.keys.is_empty() || request.keys.contains(id);
        Ok(RowSet::from_ids(
            self.catalog.iter().filter(|id| keep(id)).cloned(),
        ))
    }
}

fn local(n: usize) -> RowSet {
    RowSet::from_ids((0..n).map(|i| format!("x{i}")))
}

// spec: MEDULLA-FEDERATED-QUERY-R041
#[test]
fn an_oversized_batch_is_split_until_the_source_accepts_it() {
    let source = Scripted::new("split-test", catalog(5000), 10);
    let session = FederationSession::from_env();
    let input = local(40);
    let read = Fragment::new(&source, &session).join(&input).unwrap();
    let naive = RowSet::from_ids(catalog(5000));
    let expected = input.intersect_keep_order(&naive.id_set());
    assert_eq!(
        input.intersect_keep_order(&read.rows.id_set()),
        expected,
        "(E) the split batches return the same join"
    );
    assert_eq!(read.trace.strategy, FetchStrategy::BindJoin);
    assert_eq!(
        read.trace.requests, 6,
        "(B) 40 and 20 refused, then four batches of 10"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R041
#[test]
fn failing_key_lookups_fall_back_to_the_naive_fetch() {
    let source = Scripted::new("fallback-test", catalog(50), 0);
    let session = FederationSession::from_env();
    let input = local(20);
    let read = Fragment::new(&source, &session).join(&input).unwrap();
    assert_eq!(read.trace.strategy, FetchStrategy::FallbackFullFetch);
    assert_eq!(
        read.rows,
        RowSet::from_ids(catalog(50)),
        "(E) the naive rows"
    );
    let learned =
        super::stats::lookup(&source.identity.fingerprint).expect("the failure is remembered");
    assert_eq!(learned.key_lookup_failures, 1);
    assert_eq!(learned.full_samples, 1);
}

struct Concurrent {
    identity: Identity,
    active: AtomicUsize,
    peak: AtomicUsize,
    /// A key batch carrying this key is refused.
    poison: Option<&'static str>,
    /// What actually crossed: fetches received, and rows returned.
    calls: AtomicUsize,
    rows: AtomicUsize,
}

impl Concurrent {
    fn new() -> Self {
        Self::tagged("fo09-concurrency", None)
    }

    fn tagged(tag: &str, poison: Option<&'static str>) -> Self {
        Self {
            identity: Identity {
                label: "concurrent".into(),
                fingerprint: fingerprint(tag.as_bytes()),
                cache_name: None,
            },
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            poison,
            calls: AtomicUsize::new(0),
            rows: AtomicUsize::new(0),
        }
    }

    /// The requested keys, or the whole 200-row catalog when none are named; a batch
    /// carrying the poisoned key is refused.
    fn answer(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .poison
            .is_some_and(|key| request.keys.iter().any(|k| k == key))
        {
            return Err("concurrent: this batch is refused".into());
        }
        let ids = if request.keys.is_empty() {
            catalog(200)
        } else {
            request.keys.clone()
        };
        self.rows.fetch_add(ids.len(), Ordering::SeqCst);
        Ok(RowSet::from_ids(ids))
    }
}

impl RemoteFetch for Concurrent {
    fn capabilities(&self) -> SourceCapabilities {
        let mut caps = SourceCapabilities::single_full_fetch(
            KeyLookup::Batched { max_keys: 32 },
            LimitPushdown::Unsupported,
        );
        caps.rate = SourceRate::new(2, 0);
        caps
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn parallel_safe(&self) -> Option<&(dyn RemoteFetch + Sync)> {
        Some(self)
    }

    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(15));
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.answer(request)
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R047
#[test]
fn parallel_batches_share_a_source_cap_across_queries() {
    let source = Concurrent::new();
    let sessions = [FederationSession::from_env(), FederationSession::from_env()];
    let input = local(128);
    std::thread::scope(|scope| {
        let handles: Vec<_> = sessions
            .iter()
            .map(|session| {
                let source = &source;
                let input = &input;
                scope.spawn(move || Fragment::new(source, session).join(input).unwrap())
            })
            .collect();
        for handle in handles {
            let read = handle.join().unwrap();
            assert_eq!(read.rows, input, "parallel batches retain exact join rows");
            assert_eq!(read.trace.requests, 4);
        }
    });
    let peak = source.peak.load(Ordering::SeqCst);
    assert_eq!(
        peak, 2,
        "parallel batches use but never exceed the source cap"
    );
}

/// The first batch of the first parallel round is refused while the batch beside it is
/// answered; the sequential retries of that batch are refused too, and the fragment
/// falls back to a full fetch. The trace must count all of it.
#[test]
fn a_failed_parallel_round_counts_every_request_and_row_it_made() {
    let source = Concurrent::tagged("accounting-parallel-round", Some("x5"));
    let session = FederationSession::from_env();
    let read = Fragment::new(&source, &session).join(&local(128)).unwrap();

    assert_eq!(read.trace.strategy, FetchStrategy::FallbackFullFetch);
    assert_eq!(read.rows.len(), 200, "the fallback's rows");
    let calls = source.calls.load(Ordering::SeqCst);
    let rows = source.rows.load(Ordering::SeqCst);
    assert!(
        calls > 3 && rows > 200,
        "a parallel round with an answered batch, retries and the fallback: {calls} / {rows}"
    );
    assert_eq!(
        read.trace.requests as usize, calls,
        "every request the source received is in the trace"
    );
    assert_eq!(
        read.trace.rows_fetched, rows,
        "every row the source returned is in the trace"
    );
}

#[test]
fn a_small_learned_source_is_fetched_whole_next_time() {
    let source = Scripted::new("learned-small-test", catalog(20), 1000);
    super::stats::observe_full_fetch(&source.identity.fingerprint, 20);
    let session = FederationSession::from_env();
    let read = Fragment::new(&source, &session).join(&local(300)).unwrap();
    assert_eq!(
        read.trace.strategy,
        FetchStrategy::FullFetch,
        "20 rows beat 300 keys + a round trip"
    );
    assert!(matches!(
        read.trace.estimate,
        super::EstimateProvenance::Learned { samples: 1, .. }
    ));
}

// ── LIMIT-hint soundness ─────────────────────────────────────────────────────────

#[test]
fn a_limit_hint_is_dropped_when_occurrences_disagree() {
    let src = || Op::ForeignScan {
        source: Box::new(ForeignSourceSpec::Named { name: "s".into() }),
        join: false,
    };
    let session = FederationSession::from_env();
    session.prepare(&[src(), Op::Limit { k: 3 }]);
    assert_eq!(session.limit_hint(&src()), Some(3));
    session.prepare(&[src(), Op::Limit { k: 3 }, src(), Op::Limit { k: 100 }]);
    assert_eq!(session.limit_hint(&src()), None, "two different limits");
    session.prepare(&[
        src(),
        Op::Limit { k: 3 },
        Op::Scan {
            label: "Doc".into(),
        },
        src(),
    ]);
    assert_eq!(
        session.limit_hint(&src()),
        None,
        "an occurrence without a limit"
    );
}

// ── EXPLAIN route and per-query snapshots (MEDULLA-UNIFIED-DATA-PLANE-R038.2.3, R038.3.x) ──

mod snapshot_reads {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use medulla_exec::algebra::Op;
    use medulla_exec::rowset::RowSet;
    use medulla_types::wire::ForeignSourceSpec;

    use super::super::capability::{RemoteRequest, SourceCapabilities};
    use super::super::remote::{Identity, RemoteFetch};
    use super::super::run::Fragment;
    use super::super::snapshot::{Consistency, SnapshotBound, SnapshotConn};
    use super::super::stats::fingerprint;
    use super::super::{
        render_trace, FederationSession, FetchStrategy, FragmentExecutionMode, FragmentTrace,
        REPLICA_LAG_EXCEEDED, SNAPSHOT_UNAVAILABLE, SOURCE_WRITE_FORBIDDEN,
    };
    use medulla_types::metadata_acceleration::{SnapshotIsolation, SnapshotSemantics};

    fn spec() -> ForeignSourceSpec {
        ForeignSourceSpec::Named {
            name: "orders".into(),
        }
    }

    fn two_fragment_session() -> FederationSession {
        let session = FederationSession::from_env();
        let scan = || Op::ForeignScan {
            source: Box::new(spec()),
            join: false,
        };
        session.prepare(&[scan(), scan()]);
        session
    }

    /// The versioned table's rows are named by the version a read saw.
    struct PinnedRead {
        version: usize,
        statements: Arc<AtomicUsize>,
    }

    impl SnapshotConn for PinnedRead {
        fn run(&self, _statement: &str) -> Result<RowSet, String> {
            self.statements.fetch_add(1, Ordering::SeqCst);
            Ok(RowSet::from_ids([format!("v{}", self.version)]))
        }
    }

    /// A fake transactional source over a table whose version a writer can bump.
    struct Versioned {
        identity: Identity,
        table_version: Arc<AtomicUsize>,
        statements: Arc<AtomicUsize>,
        transactional: bool,
        replica_lag_ms: Option<u64>,
        held: Mutex<Option<Arc<dyn SnapshotConn>>>,
    }

    impl Versioned {
        fn new(table_version: &Arc<AtomicUsize>, statements: &Arc<AtomicUsize>) -> Self {
            Self {
                identity: Identity {
                    label: "sql#0badf00d".into(),
                    fingerprint: fingerprint(b"versioned"),
                    cache_name: None,
                },
                table_version: Arc::clone(table_version),
                statements: Arc::clone(statements),
                transactional: true,
                replica_lag_ms: None,
                held: Mutex::new(None),
            }
        }
    }

    impl RemoteFetch for Versioned {
        fn capabilities(&self) -> SourceCapabilities {
            let caps = SourceCapabilities::fetch_only();
            if self.transactional {
                caps.with_snapshot_reads()
            } else {
                caps
            }
        }
        fn identity(&self) -> &Identity {
            &self.identity
        }
        fn fetch(&self, _request: &RemoteRequest) -> Result<RowSet, String> {
            let held = self.held.lock().unwrap().clone();
            match held {
                Some(conn) => conn.run("SELECT id FROM t"),
                None => {
                    self.statements.fetch_add(1, Ordering::SeqCst);
                    let version = self.table_version.load(Ordering::SeqCst);
                    Ok(RowSet::from_ids([format!("v{version}")]))
                }
            }
        }
        fn open_snapshot(
            &self,
            bound: &SnapshotBound,
        ) -> Result<Option<SnapshotSemantics>, String> {
            if self
                .replica_lag_ms
                .is_some_and(|lag| lag > bound.replica_lag_bound_ms)
            {
                return Err(format!("{REPLICA_LAG_EXCEEDED}: replay lag over the bound"));
            }
            let version = self.table_version.load(Ordering::SeqCst);
            *self.held.lock().unwrap() = Some(Arc::new(PinnedRead {
                version,
                statements: Arc::clone(&self.statements),
            }));
            Ok(Some(SnapshotSemantics {
                isolation: SnapshotIsolation::RepeatableRead,
                snapshot_token: format!("snap-{version}"),
                replica_lag_bound_ms: bound.replica_lag_bound_ms,
            }))
        }
        fn held_snapshot(&self) -> Option<Arc<dyn SnapshotConn>> {
            self.held.lock().unwrap().clone()
        }
        fn adopt_snapshot(&self, conn: Arc<dyn SnapshotConn>) {
            *self.held.lock().unwrap() = Some(conn);
        }
    }

    fn read_ids(remote: &Versioned, session: &FederationSession) -> Vec<String> {
        let mut fragment = Fragment::new(remote, session);
        fragment.bind_snapshot(&spec()).unwrap();
        fragment.source(None).unwrap().rows.ids()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.3.2
    #[test]
    fn fragments_share_snapshot() {
        let (table, statements) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let session = two_fragment_session();
        let first = read_ids(&Versioned::new(&table, &statements), &session);
        table.fetch_add(1, Ordering::SeqCst); // a writer commits between the two fragments
        let second = read_ids(&Versioned::new(&table, &statements), &session);
        assert_eq!(first, vec!["v0".to_string()]);
        assert_eq!(second, first, "both fragments read the same version");
        let unbound = Versioned::new(&table, &statements);
        assert_eq!(
            unbound.fetch(&RemoteRequest::full()).unwrap().ids(),
            vec!["v1".to_string()],
            "a read outside the snapshot sees the commit"
        );
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.3.3: Wiring: `foreign_scan` opens the snapshot and the capability row declares it
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.3.3
    #[test]
    fn foreign_scan_opens_snapshot() {
        let (table, statements) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let remote = Versioned::new(&table, &statements);
        assert!(remote.capabilities().snapshot_reads);
        assert!(!SourceCapabilities::fetch_only().snapshot_reads);
        assert!(
            !SourceCapabilities::single_full_fetch(
                super::super::capability::KeyLookup::Unsupported,
                super::super::capability::LimitPushdown::Unsupported
            )
            .snapshot_reads
        );
        // One fragment of a source needs no snapshot.
        let single = FederationSession::from_env();
        single.prepare(&[Op::ForeignScan {
            source: Box::new(spec()),
            join: false,
        }]);
        Fragment::new(&remote, &single)
            .bind_snapshot(&spec())
            .unwrap();
        assert!(remote.held_snapshot().is_none());
        // Two fragments open it once.
        let session = two_fragment_session();
        Fragment::new(&remote, &session)
            .bind_snapshot(&spec())
            .unwrap();
        assert!(remote.held_snapshot().is_some());
        assert!(session.snapshot_of(&remote.fingerprint()).is_some());
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.3.4: Refusal: no snapshot, stale replica, or reachable write path
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.3.4
    #[test]
    fn snapshot_refusals() {
        let (table, statements) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        // No snapshot: two fragments of a source without snapshot_reads.
        let mut plain = Versioned::new(&table, &statements);
        plain.transactional = false;
        let session = two_fragment_session();
        let error = Fragment::new(&plain, &session)
            .bind_snapshot(&spec())
            .unwrap_err();
        assert!(error.starts_with(SNAPSHOT_UNAVAILABLE), "{error}");
        // The caller may accept per-fragment reads, and the trace records it.
        session.set_consistency(Consistency::PerFragment);
        let mut fragment = Fragment::new(&plain, &session);
        fragment.bind_snapshot(&spec()).unwrap();
        let read = fragment.source(None).unwrap();
        assert_eq!(read.trace.consistency, Consistency::PerFragment);
        assert!(render_trace(&[read.trace]).contains("consistency=per_fragment"));
        statements.store(0, Ordering::SeqCst);
        // Stale replica.
        let mut stale = Versioned::new(&table, &statements);
        stale.replica_lag_ms = Some(60_000);
        let session = two_fragment_session();
        session.set_snapshot_bound(SnapshotBound {
            replica_lag_bound_ms: 1_000,
            statement_timeout_ms: 5_000,
        });
        let error = Fragment::new(&stale, &session)
            .bind_snapshot(&spec())
            .unwrap_err();
        assert!(error.starts_with(REPLICA_LAG_EXCEEDED), "{error}");
        // A write statement is refused before any connection is used.
        #[cfg(feature = "federation-sql")]
        {
            use medulla_exec::sql_text::SqlDialect;
            let error = super::super::sql::ensure_select("DELETE FROM t", SqlDialect::Postgres)
                .unwrap_err();
            assert!(error.starts_with(SOURCE_WRITE_FORBIDDEN), "{error}");
        }
        let _ = SOURCE_WRITE_FORBIDDEN;
        assert_eq!(
            statements.load(Ordering::SeqCst),
            0,
            "no refusal ran a source statement"
        );
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.3.5: Evidence: EXPLAIN and provenance name the snapshot
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.3.5
    #[test]
    fn provenance_names_snapshot() {
        let (table, statements) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let session = two_fragment_session();
        for _ in 0..2 {
            let remote = Versioned::new(&table, &statements);
            let mut fragment = Fragment::new(&remote, &session);
            fragment.bind_snapshot(&spec()).unwrap();
            fragment.source(None).unwrap().finish_for_test(&session);
            table.fetch_add(1, Ordering::SeqCst);
        }
        let trace = session.trace();
        assert_eq!(trace.len(), 2);
        let tokens: Vec<_> = trace
            .iter()
            .map(|t| {
                t.snapshot
                    .as_ref()
                    .expect("snapshot named")
                    .snapshot_token
                    .clone()
            })
            .collect();
        assert_eq!(tokens[0], tokens[1], "one token per source per query");
        assert_eq!(trace[0].source, "sql#0badf00d");
        assert!(trace.iter().all(|t| t.read_at_ms > 0));
        assert!(render_trace(&trace).contains("snapshot=snap-0 isolation=repeatable_read"));
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.2.3: EXPLAIN states whether each fragment ran live or from a copy
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.3
    #[test]
    fn trace_names_route_per_fragment() {
        let live = FragmentTrace::new(
            "sql#0badf00d".into(),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Live,
        );
        let mut copy = FragmentTrace::new(
            "http-json#feedbeef".into(),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Copy,
        );
        copy.watermark = Some("lsn-7".into());
        copy.copy_rows = Some(3);
        let rendered = render_trace(&[live, copy]);
        let lines: Vec<_> = rendered.lines().collect();
        assert!(lines[0].ends_with("mode=live"), "{}", lines[0]);
        assert!(lines[1].ends_with("mode=copy"), "{}", lines[1]);
    }
}

// ── operation-bound MCP source through the connector port ────────────────────────

/// A connector port that records its requests and answers from a fixed reply.
struct FakePort {
    declared: Option<Vec<String>>,
    reply: Result<Vec<String>, String>,
    seen: Mutex<Vec<super::mcp::ConnectorRowRequest>>,
}

impl super::mcp::ConnectorRowSource for FakePort {
    fn declared_arguments(&self, _server: &str, _tool: &str) -> Option<Vec<String>> {
        self.declared.clone()
    }
    fn rows(
        &self,
        request: &super::mcp::ConnectorRowRequest,
    ) -> Result<super::mcp::ConnectorRows, String> {
        self.seen.lock().unwrap().push(request.clone());
        self.reply.clone().map(|ids| super::mcp::ConnectorRows {
            source_label: request.source_label.clone(),
            rows: RowSet::from_ids(ids),
            truncated: false,
            missing_source: None,
        })
    }
}

fn mcp_remote(port: Option<Arc<FakePort>>) -> super::mcp::McpRemote {
    let spec = ForeignSourceSpec::Mcp {
        server: "objects-mcp".into(),
        tool: "list_objects".into(),
        capabilities: medulla_types::wire::ForeignOperationCapabilities {
            key_filter_pushdown: true,
            ..Default::default()
        },
    };
    super::mcp::McpRemote::new(
        ("objects-mcp", "list_objects"),
        true,
        port.map(|p| p as Arc<dyn super::mcp::ConnectorRowSource>),
        Identity::of_spec(&spec, None),
    )
}

fn fake_port(declared: Option<&[&str]>, reply: Result<Vec<String>, String>) -> Arc<FakePort> {
    Arc::new(FakePort {
        declared: declared.map(|names| names.iter().map(|n| n.to_string()).collect()),
        reply,
        seen: Mutex::new(Vec::new()),
    })
}

// spec: MEDULLA-FEDERATED-QUERY-R073.3
#[test]
fn mcp_remote_delegates_to_connector_row_source() {
    let port = fake_port(Some(&["keys"]), Ok(vec!["d1".into(), "d2".into()]));
    let remote = mcp_remote(Some(port.clone()));
    let rows = remote
        .fetch(&RemoteRequest::keys(vec!["d1".into(), "d2".into()]))
        .unwrap();
    assert_eq!(ids(&rows), vec!["d1".to_string(), "d2".to_string()]);
    let seen = port.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one tool call for one request");
    assert_eq!(seen[0].server, "objects-mcp");
    assert_eq!(seen[0].tool, "list_objects");
    assert_eq!(seen[0].source_label, remote.label());
    assert_eq!(
        seen[0].pushed_filter,
        vec!["d1".to_string(), "d2".to_string()]
    );
    drop(seen);

    let refused: [(Option<Arc<FakePort>>, RemoteRequest, &str); 4] = [
        (None, RemoteRequest::full(), "SOURCE_UNREACHABLE"),
        (
            Some(fake_port(None, Ok(vec![]))),
            RemoteRequest::full(),
            "SOURCE_UNREACHABLE",
        ),
        (
            Some(fake_port(Some(&["keys"]), Err("timed out".into()))),
            RemoteRequest::full(),
            "SOURCE_UNREACHABLE",
        ),
        (
            Some(fake_port(Some(&[]), Ok(vec![]))),
            RemoteRequest::keys(vec!["d1".into()]),
            "UNSUPPORTED_SEARCH_ARGUMENT",
        ),
    ];
    for (port, request, code) in refused {
        let error = mcp_remote(port).fetch(&request).expect_err(code);
        assert!(error.starts_with(code), "{code}: {error}");
        assert!(
            error.contains("mcp#"),
            "the refusal names the source label: {error}"
        );
    }
}

// ── pushed relevance search (MEDULLA-UNIFIED-DATA-PLANE-R038.5) ──────────────────────

mod search_pushdown {
    use super::*;
    use crate::federation_opt::{
        fetch_top_k, query_digest, SearchHit, SearchPush, SearchRemote, Shortfall,
    };
    use medulla_types::search_source::{ScoreKind, SearchIndexCapability};

    /// A ten-document index that records every request it receives.
    struct Recording {
        identity: Identity,
        max_k: u32,
        seen: Mutex<Vec<RemoteRequest>>,
    }

    impl Recording {
        fn new(max_k: u32) -> Self {
            Self {
                identity: Identity {
                    label: "mcp:search#00000000".into(),
                    fingerprint: fingerprint(b"search"),
                    cache_name: None,
                },
                max_k,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl SearchRemote for Recording {
        fn identity(&self) -> &Identity {
            &self.identity
        }

        fn capability(&self) -> SearchIndexCapability {
            SearchIndexCapability {
                full_text: true,
                vector: false,
                filters: true,
                max_k: self.max_k,
                score_kind: ScoreKind::Bm25,
            }
        }

        fn fetch(&self, request: &RemoteRequest) -> Result<Vec<SearchHit>, String> {
            self.seen.lock().unwrap().push(request.clone());
            let page = request.page.expect("a pushed search is paged");
            Ok((page.offset..(page.offset + page.size).min(10))
                .map(|n| SearchHit {
                    id: format!("d{n}"),
                    score: 1.0 - n as f32 / 10.0,
                    attributes: vec![("n".into(), n.to_string())],
                })
                .collect())
        }
    }

    fn wanted(k: u32) -> SearchPush {
        SearchPush {
            text: Some("needle".into()),
            vector_space: None,
            filters: vec![("lang".into(), "en".into())],
            k,
        }
    }

    /// The local residual keeps every third document: d0, d3, d6, d9.
    fn every_third(hit: &SearchHit) -> bool {
        hit.attributes[0].1.parse::<usize>().unwrap() % 3 == 0
    }

    fn run(remote: &Recording, k: u32) -> crate::federation_opt::SearchOutcome {
        let session = FederationSession::new(FederationBudget::default());
        fetch_top_k(&session, remote, &wanted(k), &every_third).unwrap()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.5.2
    #[test]
    fn relevance_pushdown_replenishes() {
        let remote = Recording::new(100);
        let outcome = run(&remote, 3);
        let ids: Vec<_> = outcome.hits.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["d0", "d3", "d6"], "k final rows through the residual");
        assert_eq!(outcome.shortfall, None);
        let seen = remote.seen.lock().unwrap();
        let pushed: Vec<_> = seen
            .iter()
            .map(|r| (r.search.as_ref().unwrap().k, r.page.unwrap().offset))
            .collect();
        assert_eq!(pushed, [(6, 0), (6, 6)], "min(2k, max_k), paged on");
        // The operation's cap bounds the pushed k.
        let capped = Recording::new(4);
        run(&capped, 3);
        assert!(capped
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.search.as_ref().unwrap().k == 4));
        // An index that runs dry stops with the typed shortfall.
        let short = run(&Recording::new(100), 5);
        assert_eq!(short.hits.len(), 4);
        assert_eq!(
            short.shortfall,
            Some(Shortfall {
                requested: 5,
                returned: 4
            })
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.5.5
    #[test]
    fn search_receipt() {
        let remote = Recording::new(100);
        let session = FederationSession::new(FederationBudget::default());
        fetch_top_k(&session, &remote, &wanted(3), &every_third).unwrap();
        let trace = session.trace();
        assert_eq!(trace.len(), 1);
        let receipt = trace[0].search.clone().expect("a search receipt");
        let first = remote.seen.lock().unwrap()[0].search.clone().unwrap();
        assert_eq!(receipt.query_digest, query_digest(&first));
        assert_eq!(receipt.query_digest.len(), 64);
        assert!(receipt
            .query_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(
            (receipt.k, receipt.score_kind, receipt.returned),
            (6, ScoreKind::Bm25, 10)
        );
        assert_eq!(
            trace[0].source, "mcp:search#00000000",
            "the label, never an endpoint"
        );
    }
}
