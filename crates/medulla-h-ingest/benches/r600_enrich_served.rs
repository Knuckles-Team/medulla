//! MEDULLA-REPO-INGEST-R600.6 — hot-path proof of the post-commit enrichment.
//!
//! `Method::IndexRepository` → follow-on work item → claim →
//! `run_after_commit` over a 200,000-symbol fixture: the p50 must stay at most
//! 2 s, the p99 at most 5 s, and the peak resident memory at most 512 MiB.
//! The fixture comes from `scripts/gen_repo_fixture.py`; its path is read from
//! `R600_FIXTURE`.
//!
//! Run: python3 scripts/gen_repo_fixture.py /var/tmp/r600.jsonl &&
//!      R600_FIXTURE=/var/tmp/r600.jsonl cargo bench -p medulla-h-ingest \
//!      --features test-support --bench r600_enrich_served

use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_enrich::history::HistoryConfig;
use medulla_h_ingest::repository_index::enrich::{run_after_commit, FollowOn, FollowOnScope};
use medulla_h_ingest::repository_index::repository_envelope;
use medulla_protocol_types::change_envelope::ChangeEnvelope;
use medulla_protocol_types::protocol::Method;
use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexResult};

const SYMBOLS: usize = 200_000;
const SAMPLES: usize = 10;
const P50: Duration = Duration::from_secs(2);
const P99: Duration = Duration::from_secs(5);
const PEAK_BYTES: u64 = 512 * 1024 * 1024;
const INDEX_PREFIX: &str = "repository-index:";

fn rows() -> Vec<serde_json::Value> {
    let path = std::env::var("R600_FIXTURE")
        .expect("R600_FIXTURE names the file written by scripts/gen_repo_fixture.py");
    let text = std::fs::read_to_string(path).expect("the fixture is readable");
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture row"))
        .collect();
    assert_eq!(rows.len(), SYMBOLS, "the fixture holds {SYMBOLS} symbols");
    rows
}

fn node(row: &serde_json::Value) -> ExtractedNode {
    let text = |key: &str| row[key].as_str().expect("fixture field").to_string();
    ExtractedNode {
        node_id: text("id"),
        node_type: "SYMBOL".into(),
        properties: HashMap::from([
            ("symbol_type".to_string(), "Function".to_string()),
            ("name".to_string(), text("name")),
            ("qualified_symbol".to_string(), text("name")),
            ("file_path".to_string(), text("file")),
            ("language".to_string(), "python".to_string()),
        ]),
    }
}

/// The committed index envelope: a small sealed envelope whose operations are
/// replaced by the whole fixture (one served batch is bounded to 100,000
/// operations; the enrichment reads the committed rows, not the bound).
fn index_envelope(rows: &[serde_json::Value]) -> ChangeEnvelope {
    let seed = IndexResult {
        nodes: vec![node(&rows[0])],
        ..Default::default()
    };
    let mut envelope = repository_envelope(&seed).expect("a sealed index envelope");
    let template = envelope.mutation.operations[0].clone();
    let mut operations = Vec::new();
    for row in rows {
        let ExtractedNode {
            node_id,
            node_type,
            properties,
        } = node(row);
        let mut ordered: std::collections::BTreeMap<&str, &str> = properties
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        ordered.insert("type", &node_type);
        let mut op = template.clone();
        op.method = Method::AddNode {
            node_id,
            properties_msgpack: rmp_serde::to_vec_named(&ordered).expect("properties"),
        };
        operations.push(op);
    }
    for row in rows {
        let source = row["id"].as_str().expect("id");
        for target in row["calls"].as_array().expect("calls") {
            let edge = ExtractedEdge {
                source: source.to_string(),
                target: target.as_str().expect("target").to_string(),
                edge_type: "calls".into(),
                properties: HashMap::new(),
            };
            let mut op = template.clone();
            op.method = Method::AddEdge {
                source_id: edge.source,
                target_id: edge.target,
                properties_msgpack: rmp_serde::to_vec_named(&std::collections::BTreeMap::from([(
                    "relationship",
                    edge.edge_type.as_str(),
                )]))
                .expect("properties"),
            };
            operations.push(op);
        }
    }
    for (ordinal, op) in operations.iter_mut().enumerate() {
        op.ordinal = ordinal as u32;
    }
    envelope.mutation.operations = operations;
    envelope
}

fn peak_resident_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("procfs");
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .expect("VmHWM");
    kib * 1024
}

fn percentile(sorted: &[Duration], quantile: f64) -> Duration {
    let last = sorted.len() - 1;
    sorted[((last as f64) * quantile).ceil() as usize]
}

fn enrich_served(c: &mut Criterion) {
    let rows = rows();
    let envelope = index_envelope(&rows);
    drop(rows);
    let digest = envelope
        .envelope_id
        .strip_prefix(INDEX_PREFIX)
        .expect("an index envelope id")
        .to_string();
    let item = FollowOn {
        schema_version: 1,
        index_digest: digest,
        scope: FollowOnScope {
            tenant: "tenant-a".into(),
            graph: "graph-a".into(),
            repository_id: "repo".into(),
        },
    };
    let history = HistoryConfig::default();
    let run = || run_after_commit(black_box(&envelope), &item, &|_| None, &history);
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            black_box(run().expect("the enrichment runs"));
            started.elapsed()
        })
        .collect();
    samples.sort();
    assert!(percentile(&samples, 0.5) <= P50, "p50 {samples:?}");
    assert!(percentile(&samples, 0.99) <= P99, "p99 {samples:?}");
    assert!(peak_resident_bytes() <= PEAK_BYTES, "peak memory");
    let mut group = c.benchmark_group("repo_index/enrich");
    group.sample_size(10);
    group.bench_function("enrich_served", |b| b.iter(run));
    group.finish();
}

criterion_group!(benches, enrich_served);
criterion_main!(benches);
