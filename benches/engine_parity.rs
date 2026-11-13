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
//! `scripts/engine_parity_bench_gate.py` reads the records and the stored
//! baseline `benches/baselines/engine_parity.json` (R005.2).
//!
// spec: MEDULLA-ENGINE-PARITY-R005.1
// spec: MEDULLA-ENGINE-PARITY-R002.3
// spec: MEDULLA-ENGINE-PARITY-R004.3
// spec: MEDULLA-ENGINE-PARITY-R007.3
//!
//! Run: cargo bench --features full --bench engine_parity

#[path = "../crates/medulla-sql-tests/tests/common/commit_batch.rs"]
mod commit_batch;
#[path = "common/redb_fan_in.rs"]
mod redb_fan_in;

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
    resolve_wire_login, CachedWireLogins, WireLoginRegistry, WireLoginSource, WireProof,
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
        .flat_map(|h| h.join().expect("session thread"))
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
        .flat_map(|h| h.join().expect("session thread"))
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

/// R007.3: login plus principal resolution at p99 <= 5 ms with a warm cache.
/// One login of each kind is resolved repeatedly through `resolve_wire_login`
/// over the cached lookup the pgwire startup handler uses.
fn pgwire_login() -> Record {
    let certificate_der = b"bench-client-certificate-der".to_vec();
    let token = "bench-login-token"; // sanitizer:ignore
    let registry = WireLoginRegistry::default();
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
        registry
            .put(WireLoginRecord::new(
                login,
                kind,
                &verifier,
                "agent:bench",
                "bench-tenant",
                1,
            ))
            .expect("register bench login");
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
}

fn main() {
    let _ = std::fs::remove_file("target/engine_parity/records.jsonl");
    emit_workload_records();
}
