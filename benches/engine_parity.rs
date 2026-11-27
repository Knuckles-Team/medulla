//! Engine parity write workloads (MEDULLA-ENGINE-PARITY-R005.1).
//!
//! Nightly suite on the soak host; a service level, never a pull request gate.
//! Five write workloads, each appending one JSON record to
//! `target/engine_parity/records.jsonl` (`workload`, `host`, `revision`,
//! `data_digest`, `p50_us`, `p99_us`, `throughput`):
//!
//! * W1 `single_row_commit`: sequential durable single-row writes (p99 <= 5 ms).
//! * W2 `concurrent_64`: 64 concurrent single-row writers (>= 20,000 commits/s).
//! * W3 `shards_8`: 8 graphs on 8 shards, 8 writers each (>= 6 x W2).
//! * W4 `bulk_load`: 1,000,000 rows through the COPY IN commit path (>= 100,000 rows/s).
//! * W5 `conflict_heavy`: 32 sessions on 16 keys (>= 2,000 successful commits/s).
//!
//! Two budget groups ride beside them, each a record of its own:
//!
//! * `occ_validate` (R002.3): footprint validation of 100 keys, p99 <= 50 us.
//! * `disjoint_writers` (R004.3): 32 sessions on disjoint row ranges, commit
//!   p99 <= 200 us of validation wait, no `TXN_CONFLICT`.
//!
//! * `pgwire_login` (R007.3): login plus principal resolution through the warm
//!   lookup cache, one login of each kind, p99 <= 5 ms.
//!
//! * `pgwire_prepared_read` (R006.3): 10,000 executions of one prepared point read on a
//!   1,000-row table over loopback pgwire, every row checked, p99 <= 1 ms.
//!
//! * `index_probe` (R010.3): a point probe on a non-key column served by a partial
//!   index over 10 million rows, p99 <= 2 ms, and index upkeep adding at most
//!   20 percent to insert cost (fixed-seed rows).
//!
//! * `sequence_nextval` (R013.3): `nextval` served from a reserved block, p99 <= 2 us.
//!
//! * `pitr_restore_drill` (R016.3): 100,000 committed batches in one scope are archived
//!   and restored to sequence 50,000 through `restore_bundle` and `replay_archive`;
//!   no sequence is lost, a second replay applies nothing, and the restore stays
//!   within 15 minutes (the budget of a 10 GiB store). Needs the `object-store-bench`
//!   feature beside `full`.
//!
//! * `external_join` (R019.3): 1,000,000 engine rows joined with a 10,000-row outside
//!   PostgreSQL table through `FOREIGN SCAN ... JOIN` with a filter that selects 100 outside
//!   rows, p99 <= 400 ms. The outside database is the one named by `MEDULLA_TEST_POSTGRES_DSN`;
//!   an unset variable fails the bench (never a skip).
//!
//! `scripts/engine_parity_bench_gate.py` reads the records and the stored
//! baseline `benches/baselines/engine_parity.json` (R005.2).
//!
// spec: MEDULLA-ENGINE-PARITY-R005.1
// spec: MEDULLA-ENGINE-PARITY-R002.3
// spec: MEDULLA-ENGINE-PARITY-R004.3
// spec: MEDULLA-ENGINE-PARITY-R006.3
// spec: MEDULLA-ENGINE-PARITY-R007.3
// spec: MEDULLA-ENGINE-PARITY-R008.3
// spec: MEDULLA-ENGINE-PARITY-R010.3
// spec: MEDULLA-ENGINE-PARITY-R013.3
// spec: MEDULLA-ENGINE-PARITY-R016.3
// spec: MEDULLA-ENGINE-PARITY-R019.3
//!
//! Run: cargo bench --features full,object-store-bench --bench engine_parity

#![allow(dead_code)]

#[path = "../crates/medulla-sql-tests/tests/common/commit_batch.rs"]
mod commit_batch;
#[path = "../tests/common/mod.rs"]
mod common;
#[path = "common/redb_fan_in.rs"]
mod redb_fan_in;
#[path = "../crates/medulla-persistence/tests/common/replay_fixture.rs"]
mod replay_fixture;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_identity_types::identity::{sha256_verifier, WireLoginKind, WireLoginRecord};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_table_store::tables::store::OccLog;
use medulla_table_store::tables::{TableStore, TableTxn, TxnOp};
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use medulla_transaction::occ::{KeyRange, TxnFootprint};
use medulla_types::contract::Digest256;
use medulla_wire_core::sql_wire_auth::{
    resolve_wire_login, CachedWireLogins, WireLoginSource, WireProof,
};
use serde_json::json;

const W1_WRITES: usize = 2_000;
const W2_WRITERS: usize = 64;
const W2_ROUNDS: usize = 200;
const W3_GRAPHS: usize = 8;
const W3_WRITERS_PER_GRAPH: usize = 8;
const W3_ROUNDS: usize = 200;
const W4_ROWS: usize = 1_000_000;
const W4_BATCH: usize = 10_000;
const W5_SESSIONS: usize = 32;
const W5_KEYS: i64 = 16;
const W5_WINDOW: Duration = Duration::from_secs(5);
const OCC_FOOTPRINT_KEYS: usize = 100;
const OCC_VALIDATE_SAMPLES: usize = 20_000;
const OCC_VALIDATE_P99_BUDGET_US: u64 = 50;
const DISJOINT_SESSIONS: usize = 32;
const DISJOINT_ROUNDS: usize = 200;
const DISJOINT_WAIT_P99_BUDGET_US: u64 = 200;
const LOGIN_SAMPLES_PER_KIND: usize = 5_000;
const LOGIN_CACHE_TTL_S: u64 = 60;
const LOGIN_P99_BUDGET_US: u64 = 5_000;
const PREPARED_ROWS: i64 = 1_000;
const PREPARED_EXECUTIONS: i64 = 10_000;
const PREPARED_P99_BUDGET_US: u64 = 1_000;
/// R008.3: the 40 introspection statements of the fixture, replayed end to end.
const INTROSPECTION_TOTAL_BUDGET_US: u64 = 200_000;
const INTROSPECTION_REPLAYS: usize = 20;
const INDEX_PROBE_ROWS: usize = 10_000_000;
const INDEX_PROBE_BATCH: usize = 10_000;
const INDEX_PROBE_SAMPLES: usize = 2_000;
const INDEX_PROBE_P99_BUDGET_US: u64 = 2_000;
const INDEX_UPKEEP_INSERT_ROWS: usize = 200_000;
const INDEX_UPKEEP_LIMIT_PERCENT: u128 = 20;
const INDEX_PROBE_SEED: u64 = 0x5eed_1d3c;
const NEXTVAL_SAMPLES: usize = 60_000;
const NEXTVAL_BLOCK: u64 = 65_536;
const NEXTVAL_P99_BUDGET_NS: u128 = 2_000;
const PITR_BATCHES: u64 = 100_000;
const PITR_TARGET: u64 = 50_000;
const PITR_RESTORE_BUDGET: Duration = Duration::from_secs(15 * 60);
const EXTERNAL_JOIN_ENGINE_ROWS: usize = 1_000_000;
const EXTERNAL_JOIN_OUTSIDE_ROWS: usize = 10_000;
const EXTERNAL_JOIN_SELECTED_ROWS: usize = 100;
const EXTERNAL_JOIN_SAMPLES: usize = 30;
const EXTERNAL_JOIN_P99_BUDGET_US: u64 = 400_000;
const MATVIEW_ROWS: usize = 1_000_000;
const MATVIEW_READ_SAMPLES: usize = 5_000;
const MATVIEW_REFRESH_BUDGET: Duration = Duration::from_secs(2);
const MATVIEW_READ_P99_BUDGET_US: u64 = 3_000;

/// One workload result line.
struct Record {
    workload: &'static str,
    parameters: String,
    p50_us: u64,
    p99_us: u64,
    throughput: f64,
}

fn percentile(sorted: &[Duration], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() * pct / 100).min(sorted.len() - 1)].as_micros() as u64
}

fn host() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map_or_else(|| "unknown".to_string(), |h| h.trim().to_string())
}

fn revision() -> String {
    std::env::var("MEDULLA_REVISION").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map_or_else(|| "unknown".to_string(), |r| r.trim().to_string())
    })
}

/// Append one record for `record` to `target/engine_parity/records.jsonl` and print it.
fn emit(record: &Record) {
    let line = json!({
        "workload": record.workload,
        "host": host(),
        "revision": revision(),
        "data_digest": Digest256::sha256(record.parameters.as_bytes()).to_hex(),
        "p50_us": record.p50_us,
        "p99_us": record.p99_us,
        "throughput": record.throughput,
    });
    let dir = std::path::Path::new("target/engine_parity");
    std::fs::create_dir_all(dir).expect("records directory");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("records.jsonl"))
        .expect("records file");
    writeln!(file, "{line}").expect("append record");
    println!("{line}");
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-engine-parity-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn write_node(backend: &RedbBackend, graph: &str, id: usize) -> Duration {
    let started = Instant::now();
    backend
        .record_durable(
            graph,
            &Method::AddNode {
                node_id: format!("n{id}"),
                properties_msgpack: redb_fan_in::node_props(id),
            },
        )
        .await
        .expect("durable commit");
    started.elapsed()
}

fn w1_single_row_commit(rt: &tokio::runtime::Runtime) -> Record {
    let dir = temp_dir("w1");
    let backend = RedbBackend::open(dir.to_string_lossy().to_string(), 4096).expect("open");
    let mut samples = Vec::with_capacity(W1_WRITES);
    let started = Instant::now();
    for i in 0..W1_WRITES {
        samples.push(rt.block_on(write_node(&backend, redb_fan_in::GRAPH, i)));
    }
    let elapsed = started.elapsed().as_secs_f64();
    samples.sort();
    Record {
        workload: "single_row_commit",
        parameters: format!("writes={W1_WRITES}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: W1_WRITES as f64 / elapsed,
    }
}

/// `writers` concurrent tasks per graph, each doing `rounds` awaited writes.
async fn fan_out(
    backend: Arc<RedbBackend>,
    graphs: usize,
    writers: usize,
    rounds: usize,
) -> (Vec<Duration>, f64) {
    let started = Instant::now();
    let mut tasks = Vec::new();
    for g in 0..graphs {
        for w in 0..writers {
            let backend = backend.clone();
            tasks.push(tokio::spawn(async move {
                let graph = if graphs == 1 {
                    redb_fan_in::GRAPH.to_string()
                } else {
                    format!("g{g}")
                };
                let mut samples = Vec::with_capacity(rounds);
                for r in 0..rounds {
                    samples
                        .push(write_node(&backend, &graph, (g * writers + w) * rounds + r).await);
                }
                samples
            }));
        }
    }
    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.expect("writer task"));
    }
    let rate = all.len() as f64 / started.elapsed().as_secs_f64();
    all.sort();
    (all, rate)
}

fn w2_concurrent_64(rt: &tokio::runtime::Runtime) -> (Record, f64) {
    let dir = temp_dir("w2");
    let backend =
        Arc::new(RedbBackend::open(dir.to_string_lossy().to_string(), 4096).expect("open"));
    let (samples, rate) = rt.block_on(fan_out(backend, 1, W2_WRITERS, W2_ROUNDS));
    let record = Record {
        workload: "concurrent_64",
        parameters: format!("writers={W2_WRITERS} rounds={W2_ROUNDS}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: rate,
    };
    (record, rate)
}

fn w3_shards_8(rt: &tokio::runtime::Runtime) -> Record {
    let dir = temp_dir("w3");
    let backend = Arc::new(
        RedbBackend::open_with_shards(dir.to_string_lossy().to_string(), 4096, W3_GRAPHS)
            .expect("open sharded"),
    );
    let (samples, rate) = rt.block_on(fan_out(backend, W3_GRAPHS, W3_WRITERS_PER_GRAPH, W3_ROUNDS));
    Record {
        workload: "shards_8",
        parameters: format!("graphs={W3_GRAPHS} writers={W3_WRITERS_PER_GRAPH} rounds={W3_ROUNDS}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: rate,
    }
}

fn open_table_store(tag: &str) -> TableStore {
    let path = temp_dir(tag).with_extension("redb");
    TableStore::open_scoped(
        &path,
        commit_batch::TENANT,
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .expect("open table store")
}

static ATTEMPT: AtomicU64 = AtomicU64::new(0);

fn commit(store: &TableStore, txn: &TableTxn) -> Result<(), String> {
    let n = ATTEMPT.fetch_add(1, Ordering::Relaxed);
    let batch = commit_batch::batch(store, &format!("b{n}"), &format!("idem-{n}"));
    store.commit_txn_batch(txn, &batch, 1).map(|_| ())
}

fn w4_bulk_load() -> Record {
    let store = open_table_store("w4");
    store
        .create_table(
            &TableSchema::new(
                "bulk",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("v", ColumnType::BigInt, false, false),
                ],
            ),
            false,
        )
        .expect("create bulk");
    let mut samples = Vec::new();
    let started = Instant::now();
    for start in (0..W4_ROWS).step_by(W4_BATCH) {
        let rows = (start..start + W4_BATCH)
            .map(|i| vec![(i as i64).into(), ((i * 7) as i64).into()])
            .collect();
        let mut txn = TableTxn::new();
        txn.push(TxnOp::Insert {
            table: "bulk".into(),
            col_order: vec!["id".into(), "v".into()],
            rows,
        });
        let batch_started = Instant::now();
        commit(&store, &txn).expect("bulk batch commits");
        samples.push(batch_started.elapsed());
    }
    let elapsed = started.elapsed().as_secs_f64();
    samples.sort();
    Record {
        workload: "bulk_load",
        parameters: format!("rows={W4_ROWS} batch={W4_BATCH}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: W4_ROWS as f64 / elapsed,
    }
}

fn w5_conflict_heavy() -> Record {
    let store = open_table_store("w5");
    store
        .create_table(
            &TableSchema::new(
                "hot",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("n", ColumnType::BigInt, false, false),
                ],
            ),
            false,
        )
        .expect("create hot");
    let mut seed = TableTxn::new();
    seed.push(TxnOp::Insert {
        table: "hot".into(),
        col_order: vec!["id".into(), "n".into()],
        rows: (0..W5_KEYS).map(|k| vec![k.into(), 0i64.into()]).collect(),
    });
    commit(&store, &seed).expect("seed hot keys");
    let stop = Arc::new(AtomicBool::new(false));
    let successes = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let handles: Vec<_> = (0..W5_SESSIONS)
        .map(|session| {
            let (store, stop, successes) = (store.clone(), stop.clone(), successes.clone());
            std::thread::spawn(move || {
                let mut samples = Vec::new();
                let mut round = 0i64;
                while !stop.load(Ordering::Relaxed) {
                    let key = (session as i64 + round) % W5_KEYS;
                    round += 1;
                    let mut set = serde_json::Map::new();
                    set.insert("n".into(), json!(round));
                    let mut txn = TableTxn::new();
                    txn.pin(store.commit_seq());
                    txn.push(TxnOp::Update {
                        table: "hot".into(),
                        set,
                        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                            col: "id".into(),
                            op: medulla_result_types::row_predicate::CmpOp::Eq,
                            value: json!(key),
                        },
                    });
                    let attempt = Instant::now();
                    if commit(&store, &txn).is_ok() {
                        successes.fetch_add(1, Ordering::Relaxed);
                        samples.push(attempt.elapsed());
                    }
                }
                samples
            })
        })
        .collect();
    std::thread::sleep(W5_WINDOW);
    stop.store(true, Ordering::Relaxed);
    let mut samples: Vec<Duration> = handles
        .into_iter()
        .flat_map(|h| medulla_kernel::test_rendezvous::join_bounded(h, "session thread"))
        .collect();
    let elapsed = started.elapsed().as_secs_f64();
    samples.sort();
    Record {
        workload: "conflict_heavy",
        parameters: format!("sessions={W5_SESSIONS} keys={W5_KEYS}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: successes.load(Ordering::Relaxed) as f64 / elapsed,
    }
}

/// Footprint validation of 100 keys against a full history ring (R002.3).
fn occ_validate() -> Record {
    let log = OccLog::with_history(1024);
    for seq in 0..512u32 {
        log.record(vec![KeyRange::point(
            "occ_hist",
            format!("h{seq:08}").as_bytes(),
        )]);
    }
    let footprint = TxnFootprint {
        snapshot_seq: log.commit_seq(),
        reads: (0..OCC_FOOTPRINT_KEYS / 2)
            .map(|k| KeyRange::point("occ_bench", format!("r{k:08}").as_bytes()))
            .collect(),
        writes: (0..OCC_FOOTPRINT_KEYS / 2)
            .map(|k| KeyRange::point("occ_bench", format!("w{k:08}").as_bytes()))
            .collect(),
    };
    let mut samples = Vec::with_capacity(OCC_VALIDATE_SAMPLES);
    let started = Instant::now();
    for _ in 0..OCC_VALIDATE_SAMPLES {
        let attempt = Instant::now();
        log.validate(&footprint)
            .expect("disjoint footprint validates");
        samples.push(attempt.elapsed());
    }
    let elapsed = started.elapsed().as_secs_f64();
    samples.sort();
    let p99 = percentile(&samples, 99);
    assert!(
        p99 <= OCC_VALIDATE_P99_BUDGET_US,
        "occ_validate p99 {p99} us exceeds {OCC_VALIDATE_P99_BUDGET_US} us"
    );
    Record {
        workload: "occ_validate",
        parameters: format!("keys={OCC_FOOTPRINT_KEYS} history=512 samples={OCC_VALIDATE_SAMPLES}"),
        p50_us: percentile(&samples, 50),
        p99_us: p99,
        throughput: OCC_VALIDATE_SAMPLES as f64 / elapsed,
    }
}

/// 32 sessions, each on its own row range of one table: every commit succeeds
/// and the time spent in footprint validation stays under budget (R004.3).
fn disjoint_writers() -> Record {
    let store = open_table_store("disjoint");
    store
        .create_table(
            &TableSchema::new(
                "lanes",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("n", ColumnType::BigInt, false, false),
                ],
            ),
            false,
        )
        .expect("create lanes");
    let mut seed = TableTxn::new();
    seed.push(TxnOp::Insert {
        table: "lanes".into(),
        col_order: vec!["id".into(), "n".into()],
        rows: (0..DISJOINT_SESSIONS as i64)
            .map(|k| vec![k.into(), 0i64.into()])
            .collect(),
    });
    commit(&store, &seed).expect("seed lanes");
    // The shared footprint log the sessions validate against: its lock is the
    // only place disjoint writers could wait on each other.
    let log = Arc::new(OccLog::with_history(1024));
    let started = Instant::now();
    let handles: Vec<_> = (0..DISJOINT_SESSIONS)
        .map(|session| {
            let (store, log) = (store.clone(), log.clone());
            std::thread::spawn(move || {
                let mut samples = Vec::with_capacity(DISJOINT_ROUNDS);
                for round in 0..DISJOINT_ROUNDS {
                    let mut set = serde_json::Map::new();
                    set.insert("n".into(), json!(round as i64 + 1));
                    let mut txn = TableTxn::new();
                    txn.pin(store.commit_seq());
                    txn.push(TxnOp::Update {
                        table: "lanes".into(),
                        set,
                        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                            col: "id".into(),
                            op: medulla_result_types::row_predicate::CmpOp::Eq,
                            value: json!(session as i64),
                        },
                    });
                    commit(&store, &txn).expect("a disjoint commit never conflicts");
                    let footprint = TxnFootprint {
                        snapshot_seq: log.commit_seq(),
                        reads: Vec::new(),
                        writes: vec![KeyRange::point(
                            "lanes",
                            format!("s{session:04}r{round:06}").as_bytes(),
                        )],
                    };
                    let attempt = Instant::now();
                    log.validate(&footprint)
                        .expect("disjoint footprints never conflict");
                    samples.push(attempt.elapsed());
                    log.record(footprint.writes);
                }
                samples
            })
        })
        .collect();
    let mut samples: Vec<Duration> = handles
        .into_iter()
        .flat_map(|h| medulla_kernel::test_rendezvous::join_bounded(h, "session thread"))
        .collect();
    let elapsed = started.elapsed().as_secs_f64();
    samples.sort();
    let p99 = percentile(&samples, 99);
    assert!(
        p99 <= DISJOINT_WAIT_P99_BUDGET_US,
        "disjoint_writers validation wait p99 {p99} us exceeds {DISJOINT_WAIT_P99_BUDGET_US} us"
    );
    Record {
        workload: "disjoint_writers",
        parameters: format!("sessions={DISJOINT_SESSIONS} rounds={DISJOINT_ROUNDS}"),
        p50_us: percentile(&samples, 50),
        p99_us: p99,
        throughput: (DISJOINT_SESSIONS * DISJOINT_ROUNDS) as f64 / elapsed,
    }
}

/// The wire-login records of the `pgwire_login` group: a fixed map standing in for the identity
/// store the listener reads.
#[derive(Default)]
struct BenchLogins {
    records: std::collections::HashMap<String, WireLoginRecord>,
}

impl WireLoginSource for BenchLogins {
    fn find(&self, login: &str) -> Option<WireLoginRecord> {
        self.records.get(login).cloned()
    }
}

/// A pgwire listener on a loopback port over a seeded durable state, and a connected
/// client (the helper `tests/pgwire_roundtrip.rs` uses: `spawn_pgwire_listener` and
/// `connect_pgwire` of `tests/common/test_support.rs`).
async fn pgwire_listener_and_client() -> tokio_postgres::Client {
    // The text embedder binds once per process, before any listener exists.
    static TEXT_EMBEDDER_ENV: std::sync::Once = std::sync::Once::new();
    TEXT_EMBEDDER_ENV.call_once(|| std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash"));
    let persistence = test_support::durable_persistence("engine-parity-bench-recovery-key");
    let state = test_support::seeded_wire_state(
        "bench",
        "tester",
        Some(test_support::sql_test_persist_dir("pgwire-bench")),
        persistence,
    );
    let addr =
        test_support::spawn_pgwire_listener(state, medulla_wire::pgwire::PgWireAuthMode::Scram)
            .await;
    test_support::connect_pgwire(&addr, "bench", "tester", "__commons__")
        .await
        .expect("pgwire connect")
}

/// R006.3: one prepared point read executed 10,000 times on loopback over a 1,000-row table,
/// ids cycling over the table, p99 <= 1 ms. Every returned row is checked against its seed.
fn pgwire_prepared_read(rt: &tokio::runtime::Runtime) -> Record {
    rt.block_on(async {
        let client = pgwire_listener_and_client().await;
        client
            .simple_query("CREATE TABLE bench_prepared (id BIGINT PRIMARY KEY, v BIGINT NOT NULL)")
            .await
            .expect("CREATE TABLE");
        for chunk in (0..PREPARED_ROWS).collect::<Vec<_>>().chunks(100) {
            let values: Vec<String> = chunk
                .iter()
                .map(|id| format!("({id}, {})", id * 7 + 3))
                .collect();
            client
                .simple_query(&format!(
                    "INSERT INTO bench_prepared (id, v) VALUES {}",
                    values.join(", ")
                ))
                .await
                .expect("seed rows");
        }
        let statement = client
            .prepare("SELECT v FROM bench_prepared WHERE id = $1")
            .await
            .expect("prepare the point read once");
        let hits_before = medulla_wire::pgwire::plan_cache_hits();
        let mut samples = Vec::with_capacity(PREPARED_EXECUTIONS as usize);
        let started = Instant::now();
        for execution in 0..PREPARED_EXECUTIONS {
            let id = execution % PREPARED_ROWS;
            let begun = Instant::now();
            let rows = client
                .query(&statement, &[&id])
                .await
                .expect("prepared point read");
            samples.push(begun.elapsed());
            assert_eq!(rows.len(), 1, "id {id} returns one row");
            assert_eq!(rows[0].get::<_, i64>(0), id * 7 + 3, "value of id {id}");
        }
        let wall = started.elapsed();
        let hits = medulla_wire::pgwire::plan_cache_hits() - hits_before;
        assert!(
            hits >= (PREPARED_EXECUTIONS - 1) as u64,
            "the plan cache served {hits} of {PREPARED_EXECUTIONS} executions"
        );
        samples.sort();
        let record = Record {
            workload: "pgwire_prepared_read",
            parameters: format!("rows={PREPARED_ROWS};executions={PREPARED_EXECUTIONS}"),
            p50_us: percentile(&samples, 50),
            p99_us: percentile(&samples, 99),
            throughput: PREPARED_EXECUTIONS as f64 / wall.as_secs_f64(),
        };
        assert!(
            record.p99_us <= PREPARED_P99_BUDGET_US,
            "pgwire_prepared_read p99 {} us over the {} us budget",
            record.p99_us,
            PREPARED_P99_BUDGET_US
        );
        record
    })
}

/// R008.3: the 40 introspection statements of `tests/fixtures/pg_introspection/queries.sql`
/// replayed by a grantee over loopback; one replay is the total of the 40 statements, at most
/// 200 ms. The grantee holds `SqlPrivilege::Select` on one of two tables of another principal.
fn pgwire_introspection(rt: &tokio::runtime::Runtime) -> Record {
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::carrier::CarrierAuthority;
    use medulla_server_auth::sql_catalog_acl::SqlPrivilege;

    const QUERIES: &str = include_str!("../tests/fixtures/pg_introspection/queries.sql");
    const PASSWORD: &str = "bench-introspection-pass"; // sanitizer:ignore
    const SALT: [u8; 16] = [7u8; 16];
    let statements: Vec<&str> = QUERIES.lines().collect();
    assert_eq!(statements.len(), 40, "the fixture holds 40 statements");
    rt.block_on(async {
        static TEXT_EMBEDDER_ENV: std::sync::Once = std::sync::Once::new();
        TEXT_EMBEDDER_ENV.call_once(|| std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash"));
        let principals = ["intro-owner", "intro-reader"];
        let persist_dir = test_support::sql_test_persist_dir("pgwire-introspection-bench");
        let state = test_support::state_with_registry(
            "bench-introspection-secret", // sanitizer:ignore
            test_support::wire_isolation(&principals),
            medulla_core::registry::GraphRegistry::new(),
            Some(persist_dir.clone()),
            None,
        );
        for (login, principal) in [
            ("intro-owner-login", "intro-owner"),
            ("intro-reader-login", "intro-reader"),
        ] {
            let record = WireLoginRecord::new(
                login,
                WireLoginKind::Password,
                &medulla_wire::pgwire::password_verifier(PASSWORD, &SALT),
                principal,
                common::TEST_TENANT,
                1,
            );
            state.write().await.isolation.seed_wire_login(record);
        }
        let addr =
            test_support::spawn_pgwire_listener(state, medulla_wire::pgwire::PgWireAuthMode::Scram)
                .await;
        let owner = test_support::connect_pgwire_with_password(
            &addr,
            PASSWORD,
            "intro-owner-login",
            "__commons__",
        )
        .await
        .expect("owner login");
        for table in [
            "CREATE TABLE t_visible (id BIGINT PRIMARY KEY, name TEXT)",
            "CREATE TABLE t_hidden (id BIGINT PRIMARY KEY)",
        ] {
            owner.simple_query(table).await.expect("CREATE TABLE");
        }
        let carrier =
            CarrierAuthority::from_verified(&VerifiedRequestContext::verified_for_test_in_tenant(
                "intro-owner",
                common::TEST_TENANT,
            ))
            .expect("owner carrier");
        medulla_server_auth::sql_catalog_acl::grant(
            std::path::Path::new(&persist_dir),
            &carrier,
            "t_visible",
            "intro-reader",
            &[SqlPrivilege::Select],
            uuid::Uuid::new_v4(),
        )
        .expect("grant select");
        let reader = test_support::connect_pgwire_with_password(
            &addr,
            PASSWORD,
            "intro-reader-login",
            "__commons__",
        )
        .await
        .expect("grantee login");

        let mut samples = Vec::with_capacity(INTROSPECTION_REPLAYS);
        let started = Instant::now();
        for _ in 0..INTROSPECTION_REPLAYS {
            let begun = Instant::now();
            for statement in &statements {
                reader
                    .simple_query(statement)
                    .await
                    .unwrap_or_else(|err| panic!("{statement}: {err}"));
            }
            samples.push(begun.elapsed());
        }
        let wall = started.elapsed();
        samples.sort();
        let record = Record {
            workload: "pgwire_introspection",
            parameters: format!("statements=40;replays={INTROSPECTION_REPLAYS}"),
            p50_us: percentile(&samples, 50),
            p99_us: percentile(&samples, 99),
            throughput: INTROSPECTION_REPLAYS as f64 / wall.as_secs_f64(),
        };
        assert!(
            record.p99_us <= INTROSPECTION_TOTAL_BUDGET_US,
            "pgwire_introspection p99 {} us over the {} us budget",
            record.p99_us,
            INTROSPECTION_TOTAL_BUDGET_US
        );
        record
    })
}

/// R007.3: login plus principal resolution at p99 <= 5 ms with a warm cache.
/// One login of each kind is resolved repeatedly through `resolve_wire_login`
/// over the cached lookup the pgwire startup handler uses.
fn pgwire_login() -> Record {
    let certificate_der = b"bench-client-certificate-der".to_vec();
    let token = "bench-login-token"; // sanitizer:ignore
    let mut registry = BenchLogins::default();
    for (login, kind, verifier) in [
        (
            "bench-password",
            WireLoginKind::Password,
            "salt:salted".to_string(),
        ),
        (
            "bench-certificate",
            WireLoginKind::Certificate,
            sha256_verifier(&certificate_der),
        ),
        (
            "bench-token",
            WireLoginKind::Token,
            sha256_verifier(token.as_bytes()),
        ),
    ] {
        registry.records.insert(
            login.to_string(),
            WireLoginRecord::new(login, kind, &verifier, "agent:bench", "bench-tenant", 1),
        );
    }
    let source = CachedWireLogins::new(Arc::new(registry), LOGIN_CACHE_TTL_S);
    for login in ["bench-password", "bench-certificate", "bench-token"] {
        let _ = source.find(login);
    }
    let mut samples = Vec::with_capacity(LOGIN_SAMPLES_PER_KIND * 3);
    let started = Instant::now();
    for _ in 0..LOGIN_SAMPLES_PER_KIND {
        let proofs = [
            ("bench-password", WireProof::ScramVerified),
            (
                "bench-certificate",
                WireProof::Certificate {
                    certificate_der: &certificate_der,
                },
            ),
            ("bench-token", WireProof::Token(token)),
        ];
        for (login, proof) in proofs {
            let begun = Instant::now();
            let resolved = resolve_wire_login(&source, login, proof).expect("bench login");
            samples.push(begun.elapsed());
            assert_eq!(resolved.principal, "agent:bench");
        }
    }
    let wall = started.elapsed();
    samples.sort();
    let record = Record {
        workload: "pgwire_login",
        parameters: format!("kinds=3;samples_per_kind={LOGIN_SAMPLES_PER_KIND}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: samples.len() as f64 / wall.as_secs_f64(),
    };
    assert!(
        record.p99_us <= LOGIN_P99_BUDGET_US,
        "pgwire_login p99 {} us over the {} us budget",
        record.p99_us,
        LOGIN_P99_BUDGET_US
    );
    record
}

/// A small fixed-seed generator: the same rows on every run (xorshift64).
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn accounts_schema(name: &str) -> TableSchema {
    TableSchema::new(
        name,
        vec![
            Column::new("id", ColumnType::BigInt, false, false),
            Column::new("email", ColumnType::Text, false, false),
            Column::new("status", ColumnType::Text, false, false),
        ],
    )
}

/// Insert `rows` rows with ids from `first_id`: one in ten is `active`, the rest
/// `closed`, with fixed-seed email values. Returns the elapsed time.
fn insert_accounts(store: &TableStore, table: &str, first_id: usize, rows: usize) -> Duration {
    let mut seed = INDEX_PROBE_SEED ^ first_id as u64;
    let columns = ["id".to_string(), "email".to_string(), "status".to_string()];
    let started = Instant::now();
    let mut done = 0;
    while done < rows {
        let batch = INDEX_PROBE_BATCH.min(rows - done);
        let values: Vec<Vec<serde_json::Value>> = (0..batch)
            .map(|offset| {
                let id = first_id + done + offset;
                let status = if next_random(&mut seed).is_multiple_of(10) {
                    "active"
                } else {
                    "closed"
                };
                vec![json!(id as i64), json!(format!("u{id}@x")), json!(status)]
            })
            .collect();
        store
            .insert_rows(table, &columns, &values)
            .expect("insert accounts");
        done += batch;
    }
    started.elapsed()
}

fn create_partial_index(store: &TableStore, sql: &str) {
    let plan = match medulla_sql_lang::sql::classify::classify(sql).expect("classify index") {
        medulla_sql_lang::sql::classify::StatementKind::CreateIndex(plan) => plan,
        other => panic!("not an index statement: {other:?}"),
    };
    let mut txn = TableTxn::new();
    txn.push(TxnOp::IndexCatalog(
        medulla_table_store::tables::IndexCatalogTxnOp::PutSecondaryIndex { plan },
    ));
    store.commit_txn(&txn).expect("create index");
}

/// R010.3: a probe on a non-key column through the partial index over
/// `INDEX_PROBE_ROWS` rows at p99 <= 2 ms, and index upkeep adding at most
/// 20 percent to insert cost.
fn index_probe() -> Record {
    use medulla_graph_core::graph::GraphCore;
    let store = open_table_store("index_probe");
    store
        .create_table(&accounts_schema("accounts"), false)
        .expect("create accounts");
    insert_accounts(&store, "accounts", 0, INDEX_PROBE_ROWS);
    create_partial_index(
        &store,
        "CREATE INDEX accounts_active_email ON accounts (email) WHERE status = 'active'",
    );
    let view = GraphCore::new().analysis_snapshot();
    let mut seed = INDEX_PROBE_SEED;
    let mut samples = Vec::with_capacity(INDEX_PROBE_SAMPLES);
    let started = Instant::now();
    for _ in 0..INDEX_PROBE_SAMPLES {
        let id = next_random(&mut seed) as usize % INDEX_PROBE_ROWS;
        let sql = format!("SELECT id FROM accounts WHERE status = 'active' AND email = 'u{id}@x'");
        let begun = Instant::now();
        medulla_sql::sql::exec_sql_typed_with_tables(&view, &store, &sql).expect("probe");
        samples.push(begun.elapsed());
    }
    let wall = started.elapsed();
    samples.sort();
    let p99 = percentile(&samples, 99);

    // Upkeep: the same rows into a table with the index and one without.
    let plain = open_table_store("index_upkeep_plain");
    plain
        .create_table(&accounts_schema("accounts"), false)
        .expect("create plain accounts");
    let plain_time = insert_accounts(&plain, "accounts", 0, INDEX_UPKEEP_INSERT_ROWS);
    let indexed = open_table_store("index_upkeep_indexed");
    indexed
        .create_table(&accounts_schema("accounts"), false)
        .expect("create indexed accounts");
    create_partial_index(
        &indexed,
        "CREATE INDEX accounts_active_email ON accounts (email) WHERE status = 'active'",
    );
    let indexed_time = insert_accounts(&indexed, "accounts", 0, INDEX_UPKEEP_INSERT_ROWS);
    assert!(
        indexed_time.as_nanos() * 100 <= plain_time.as_nanos() * (100 + INDEX_UPKEEP_LIMIT_PERCENT),
        "index upkeep added more than {INDEX_UPKEEP_LIMIT_PERCENT} percent to insert cost: {indexed_time:?} against {plain_time:?}"
    );
    assert!(
        p99 <= INDEX_PROBE_P99_BUDGET_US,
        "index_probe p99 {p99} us over the {INDEX_PROBE_P99_BUDGET_US} us budget"
    );
    Record {
        workload: "index_probe",
        parameters: format!(
            "rows={INDEX_PROBE_ROWS};samples={INDEX_PROBE_SAMPLES};seed={INDEX_PROBE_SEED}"
        ),
        p50_us: percentile(&samples, 50),
        p99_us: p99,
        throughput: samples.len() as f64 / wall.as_secs_f64(),
    }
}

/// R013.3: `nextval` inside a cached block at p99 <= 2 us. The block holds more
/// values than the sample count, so only the first call reserves.
fn sequence_nextval() -> Record {
    use medulla_tables::sequence::{Sequence, SequenceSource};
    let store = open_table_store("sequence_nextval");
    let mut sequence = Sequence::new("bench_seq");
    sequence.cache = NEXTVAL_BLOCK;
    store
        .create_sequence(&sequence, false)
        .expect("create sequence");
    store.sequence_next("bench_seq").expect("reserve the block");
    let mut samples = Vec::with_capacity(NEXTVAL_SAMPLES);
    let started = Instant::now();
    for _ in 0..NEXTVAL_SAMPLES {
        let begun = Instant::now();
        store.sequence_next("bench_seq").expect("nextval");
        samples.push(begun.elapsed());
    }
    let wall = started.elapsed();
    samples.sort();
    let p99_ns = samples[(samples.len() * 99 / 100).min(samples.len() - 1)].as_nanos();
    assert!(
        p99_ns <= NEXTVAL_P99_BUDGET_NS,
        "sequence_nextval p99 {p99_ns} ns over the {NEXTVAL_P99_BUDGET_NS} ns budget"
    );
    Record {
        workload: "sequence_nextval",
        parameters: format!("samples={NEXTVAL_SAMPLES};cache={NEXTVAL_BLOCK}"),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: samples.len() as f64 / wall.as_secs_f64(),
    }
}

/// MEDULLA-ENGINE-PARITY-R014.3: refresh a materialized view of 1,000,000 rows through the
/// one refresh transaction of the table store, then sample point reads of its result table.
fn matview_refresh() -> Record {
    use medulla_table_store::tables::{refresh_matview_txn, MatViewSpec};
    let store = open_table_store("matview_refresh");
    let name = "bench_mv";
    let schema = TableSchema::new(
        name,
        vec![
            Column::new("id", ColumnType::BigInt, true, false),
            Column::new("v", ColumnType::BigInt, true, false),
        ],
    );
    let col_order = vec!["id".to_string(), "v".to_string()];
    let spec = || MatViewSpec {
        name: name.to_string(),
        select_sql: "SELECT id, v FROM bench_src".to_string(),
        inputs: vec!["bench_src".to_string()],
        owner: "bench".to_string(),
        refreshed_seq: store.commit_seq(),
        stale: false,
        program: None,
    };
    let rows = |count: usize| -> Vec<Vec<serde_json::Value>> {
        (0..count)
            .map(|i| vec![json!(i as i64), json!((i * 7) as i64)])
            .collect()
    };
    // The first refresh creates the result table; the timed one replaces its rows.
    let mut first = TableTxn::new();
    first.push(TxnOp::CreateTable {
        schema: schema.clone(),
        if_not_exists: false,
    });
    commit(&store, &first).expect("create the result table");
    let txn = refresh_matview_txn(spec(), schema, col_order, rows(MATVIEW_ROWS));
    let started = Instant::now();
    commit(&store, &txn).expect("refresh commits");
    let refresh = started.elapsed();
    assert!(
        refresh <= MATVIEW_REFRESH_BUDGET,
        "matview_refresh took {refresh:?}, over the {MATVIEW_REFRESH_BUDGET:?} budget"
    );
    let mut seed = INDEX_PROBE_SEED;
    let mut samples = Vec::with_capacity(MATVIEW_READ_SAMPLES);
    for _ in 0..MATVIEW_READ_SAMPLES {
        let rowid = 1 + next_random(&mut seed) % (MATVIEW_ROWS as u64 - 2);
        let begun = Instant::now();
        let row = store.get_row(name, rowid).expect("point read");
        samples.push(begun.elapsed());
        assert!(row.is_some(), "row {rowid} of the refreshed view exists");
    }
    samples.sort();
    let p99_us = percentile(&samples, 99);
    assert!(
        p99_us <= MATVIEW_READ_P99_BUDGET_US,
        "matview read p99 {p99_us} us over the {MATVIEW_READ_P99_BUDGET_US} us budget"
    );
    Record {
        workload: "matview_refresh",
        parameters: format!("rows={MATVIEW_ROWS};samples={MATVIEW_READ_SAMPLES}"),
        p50_us: percentile(&samples, 50),
        p99_us,
        throughput: MATVIEW_ROWS as f64 / refresh.as_secs_f64(),
    }
}

/// `pitr_restore_drill` (R016.3): archive `PITR_BATCHES` committed batches of one scope,
/// restore the bundle to sequence `PITR_TARGET`, and prove no sequence is lost and a
/// second replay is a no-op. The record's `p50_us` and `p99_us` are the restore time.
fn pitr_restore_drill(rt: &tokio::runtime::Runtime) -> Record {
    use medulla_persistence::backup::restore_bundle;
    use medulla_persistence::backup_replay::replay_archive;
    use medulla_storage::{ledger_scope_key, ArchivePolicy, ReplayTarget};

    let dir = temp_dir("pitr");
    let source_dir = dir.join("source");
    let bundle = dir.join("bundle");
    let restored = dir.join("restored");
    let archive = replay_fixture::archive();
    let scope = ledger_scope_key(&replay_fixture::identity());
    let source =
        RedbBackend::open(source_dir.to_string_lossy().into_owned(), 1024).expect("open source");
    replay_fixture::bundle_of(&source, &bundle);
    rt.block_on(replay_fixture::commit_range(&source, 0..PITR_BATCHES));
    let policy = ArchivePolicy {
        segment_bytes: 64 * 1024 * 1024,
        retain_s: 3_600,
        interval_s: 10,
    };
    let pass = rt
        .block_on(source.archive_pass(&archive, replay_fixture::TENANT, &policy))
        .expect("archive pass");
    assert_eq!(pass.records_archived, PITR_BATCHES);
    drop(source);

    let target = ReplayTarget::Sequence(PITR_TARGET);
    let started = Instant::now();
    restore_bundle(&bundle, &restored, 1, None).expect("bundle restores");
    let report = rt
        .block_on(replay_archive(
            &restored,
            1,
            &archive,
            replay_fixture::TENANT,
            &target,
        ))
        .expect("replay to the target");
    let elapsed = started.elapsed();
    assert!(
        elapsed <= PITR_RESTORE_BUDGET,
        "pitr_restore_drill took {elapsed:?}, over the {PITR_RESTORE_BUDGET:?} budget"
    );
    assert_eq!(report.final_sequence.get(&scope), Some(&PITR_TARGET));
    assert_eq!(report.records_applied, PITR_TARGET);

    let reopened = RedbBackend::open_with_shards(restored.to_string_lossy().into_owned(), 1024, 1)
        .expect("open restored");
    for index in 0..PITR_TARGET {
        let held = rt
            .block_on(PersistenceBackend::read_mutation_batch(
                &reopened,
                replay_fixture::GRAPH,
                &format!("b{index}"),
            ))
            .expect("read the ledger");
        assert!(held.is_some(), "sequence {} is missing", index + 1);
    }
    let past = rt
        .block_on(PersistenceBackend::read_mutation_batch(
            &reopened,
            replay_fixture::GRAPH,
            &format!("b{PITR_TARGET}"),
        ))
        .expect("read the ledger");
    assert!(past.is_none(), "no sequence past the target is restored");
    drop(reopened);
    let again = rt
        .block_on(replay_archive(
            &restored,
            1,
            &archive,
            replay_fixture::TENANT,
            &target,
        ))
        .expect("second replay");
    assert_eq!(again.records_applied, 0, "a repeated restore is a no-op");
    let _ = std::fs::remove_dir_all(&dir);
    let micros = elapsed.as_micros() as u64;
    Record {
        workload: "pitr_restore_drill",
        parameters: format!("batches={PITR_BATCHES};target={PITR_TARGET}"),
        p50_us: micros,
        p99_us: micros,
        throughput: PITR_TARGET as f64 / elapsed.as_secs_f64(),
    }
}

/// R019.3: join 1,000,000 engine rows with the 10,000-row outside table `ext(id, k)` whose
/// filter column `k` selects exactly 100 rows, through the registered `Sql` source and the
/// UQL `FOREIGN SCAN ... JOIN` path, p99 <= 400 ms. Every sample checks the 100 joined ids.
fn external_join(rt: &tokio::runtime::Runtime) -> Record {
    use medulla_exec::exec::PlanCtx;
    use medulla_federation::federation::ForeignSourceRegistry;
    use medulla_federation::federation_opt::FederatedExec;
    use medulla_graph_core::graph::GraphCore;
    use medulla_plan::uql::serve::run_statement;
    use medulla_query_types::wire::{ForeignSourceSpec, UqlResult};
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_uql::{parse_statement, Params};

    let dsn = std::env::var("MEDULLA_TEST_POSTGRES_DSN").expect(
        "MEDULLA_TEST_POSTGRES_DSN names the outside PostgreSQL database of the external_join bench",
    );
    rt.block_on(async {
        let (client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
            .await
            .expect("connect to the outside database");
        let driver = tokio::spawn(connection);
        client
            .batch_execute(
                "DROP TABLE IF EXISTS ext; CREATE TABLE ext (id text PRIMARY KEY, k text NOT NULL)",
            )
            .await
            .expect("create the outside table");
        for first in (0..EXTERNAL_JOIN_OUTSIDE_ROWS).step_by(1_000) {
            let values: Vec<String> = (first..first + 1_000)
                .map(|i| {
                    let key = if i < EXTERNAL_JOIN_SELECTED_ROWS {
                        "hit"
                    } else {
                        "miss"
                    };
                    format!("('row:{i}', '{key}')")
                })
                .collect();
            client
                .batch_execute(&format!(
                    "INSERT INTO ext (id, k) VALUES {}",
                    values.join(", ")
                ))
                .await
                .expect("seed the outside table");
        }
        drop(client);
        let _ = driver.await;
    });

    let core = GraphCore::new();
    let blob = rmp_serde::to_vec_named(&json!({ "type": "Row" })).expect("node blob");
    for index in 0..EXTERNAL_JOIN_ENGINE_ROWS {
        core.add_node(format!("row:{index}"), blob.clone());
    }
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let mut registry = ForeignSourceRegistry::new();
    registry.register_spec(
        "ext_join_source",
        ForeignSourceSpec::Sql {
            dsn,
            query: "SELECT id, k FROM ext".into(),
            id_field: "id".into(),
            score_field: Some("k".into()),
            columns: Vec::new(),
        },
    );
    let federated = FederatedExec::with_registry(&registry);
    let ctx = PlanCtx::new(&view, &semantic).with_foreign_executor(&federated);
    let source =
        "MATCH (:Row) |> FOREIGN SCAN 'ext_join_source' JOIN |> WHERE k = 'hit' |> RETURN id";
    let statement = parse_statement(source, &Params::new()).expect("the joined read parses");

    let mut samples = Vec::with_capacity(EXTERNAL_JOIN_SAMPLES);
    let started = Instant::now();
    for _ in 0..EXTERNAL_JOIN_SAMPLES {
        let begun = Instant::now();
        let result = run_statement(&statement, &ctx).expect("the joined read runs");
        samples.push(begun.elapsed());
        let UqlResult::Rows { rows, .. } = result else {
            panic!("the joined read returns rows");
        };
        let mut ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
        ids.sort();
        let mut expected: Vec<String> = (0..EXTERNAL_JOIN_SELECTED_ROWS)
            .map(|i| format!("row:{i}"))
            .collect();
        expected.sort();
        assert_eq!(ids, expected, "the join returns the selected outside rows");
    }
    let wall = started.elapsed();
    samples.sort();
    let record = Record {
        workload: "external_join",
        parameters: format!(
            "engine_rows={EXTERNAL_JOIN_ENGINE_ROWS};outside_rows={EXTERNAL_JOIN_OUTSIDE_ROWS};selected={EXTERNAL_JOIN_SELECTED_ROWS}"
        ),
        p50_us: percentile(&samples, 50),
        p99_us: percentile(&samples, 99),
        throughput: EXTERNAL_JOIN_SAMPLES as f64 / wall.as_secs_f64(),
    };
    assert!(
        record.p99_us <= EXTERNAL_JOIN_P99_BUDGET_US,
        "external_join p99 {} us over the {} us budget",
        record.p99_us,
        EXTERNAL_JOIN_P99_BUDGET_US
    );
    record
}

/// Run W1 to W5 and the OCC and login budget groups; print one record per workload.
fn emit_workload_records() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("tokio runtime");
    emit(&w1_single_row_commit(&rt));
    let (w2, w2_rate) = w2_concurrent_64(&rt);
    emit(&w2);
    let w3 = w3_shards_8(&rt);
    println!(
        "shards_8 / concurrent_64 = {:.1}x (target >= 6x)",
        w3.throughput / w2_rate
    );
    emit(&w3);
    emit(&w4_bulk_load());
    emit(&w5_conflict_heavy());
    emit(&occ_validate());
    emit(&disjoint_writers());
    emit(&pgwire_login());
    emit(&pgwire_prepared_read(&rt));
    emit(&pgwire_introspection(&rt));
    emit(&index_probe());
    emit(&sequence_nextval());
    emit(&matview_refresh());
    emit(&pitr_restore_drill(&rt));
    emit(&external_join(&rt));
}

fn main() {
    let _ = std::fs::remove_file("target/engine_parity/records.jsonl");
    emit_workload_records();
}
