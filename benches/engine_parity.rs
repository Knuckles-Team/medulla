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
//! `scripts/engine_parity_bench_gate.py` reads the records and the stored
//! baseline `benches/baselines/engine_parity.json` (R005.2).
//!
//! spec: MEDULLA-ENGINE-PARITY-R005.1
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

use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_table_store::tables::{TableStore, TableTxn, TxnOp};
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use medulla_types::contract::Digest256;
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

/// Run W1 to W5 and print one record per workload.
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
}

fn main() {
    let _ = std::fs::remove_file("target/engine_parity/records.jsonl");
    emit_workload_records();
}
