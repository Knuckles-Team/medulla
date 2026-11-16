//! Hot-path journey of the domain pack mapping evaluator (MEDULLA-TYPED-PACKS-R097.701).
//!
//! Nightly bench group `domain_pack/mapping`: evaluates a three-mapping
//! manifest over 1,000 source rows through `evaluate_mapping` (the function
//! `DomainPackOp::Run` calls), and budgets p50 under 10 ms and p99 under 60 ms
//! per 1,000 rows with the proposed facts under 64 MiB. Records the figures into
//! `target/domain-pack-mapping-report.json`; `scripts/bench_gate.py` consumes
//! the group. The nightly job runs it; it never gates a pull request.
//!
//! Run: cargo bench --bench domain_pack_mapping
//!
//! spec: MEDULLA-TYPED-PACKS-R097.701

use std::time::Instant;

use medulla_catalog_types::domain_pack::{
    DomainPackManifest, ManifestClass, ManifestMapping, SourceKind, SourceRow,
};
use medulla_core::domain_pack::evaluate_mapping;

const ROWS: usize = 1_000;
const RUNS: usize = 200;
const WARMUP: usize = 20;
const P50_BUDGET_MS: f64 = 10.0;
const P99_BUDGET_MS: f64 = 60.0;
const MEMORY_BUDGET_BYTES: usize = 64 * 1024 * 1024;

fn mapping(selector: &str, target: &str) -> ManifestMapping {
    ManifestMapping {
        source_kind: SourceKind::Column,
        selector: selector.into(),
        target_class: "Person".into(),
        target_property_or_relation: target.into(),
    }
}

fn manifest() -> DomainPackManifest {
    let mut manifest = DomainPackManifest {
        schema_version: 1,
        pack_id: "hr".into(),
        version: "1".into(),
        integrity_sha256: String::new(),
        classes: vec![ManifestClass {
            name: "Person".into(),
            properties: vec!["name".into(), "email".into()],
            relations: vec!["reportsTo".into()],
        }],
        mappings: vec![
            mapping("full_name", "name"),
            mapping("mail", "email"),
            mapping("manager", "reportsTo"),
        ],
        cases: vec![],
    };
    manifest.integrity_sha256 = manifest.compute_integrity();
    manifest
}

fn rows() -> Vec<SourceRow> {
    (0..ROWS)
        .map(|index| SourceRow {
            row_id: format!("r{index:05}"),
            source_kind: SourceKind::Column,
            fields: [
                (
                    "full_name".to_string(),
                    serde_json::json!(format!("Person {index}")),
                ),
                (
                    "mail".to_string(),
                    serde_json::json!(format!("p{index}@example.org")),
                ),
                (
                    "manager".to_string(),
                    serde_json::json!(format!("hr:Person:r{:05}", index / 10)),
                ),
            ]
            .into(),
        })
        .collect()
}

fn percentile(sorted: &[f64], percent: f64) -> f64 {
    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn main() {
    let manifest = manifest();
    let rows = rows();
    for _ in 0..WARMUP {
        evaluate_mapping(&manifest, &rows);
    }
    let mut samples_ms = Vec::with_capacity(RUNS);
    let mut fact_count = 0usize;
    let mut fact_bytes = 0usize;
    for _ in 0..RUNS {
        let started = Instant::now();
        let facts = evaluate_mapping(&manifest, &rows);
        samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        fact_count = facts.len();
        fact_bytes = serde_json::to_vec(&facts).expect("encode facts").len();
    }
    assert_eq!(fact_count, ROWS * 4, "one entity and three facts per row");
    samples_ms.sort_by(|a, b| a.partial_cmp(b).expect("durations order"));
    let p50 = percentile(&samples_ms, 50.0);
    let p99 = percentile(&samples_ms, 99.0);

    let body = serde_json::json!({
        "groups": {
            "domain_pack/mapping": {
                "rows": ROWS,
                "runs": RUNS,
                "facts": fact_count,
                "fact_bytes": fact_bytes,
                "p50_ms": p50,
                "p99_ms": p99,
            }
        }
    });
    let path = "target/domain-pack-mapping-report.json";
    std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap())
        .expect("write domain pack mapping report");
    println!(
        "domain_pack/mapping: {ROWS} rows, {fact_count} facts ({fact_bytes} bytes) - \
         p50 {p50:.3} ms, p99 {p99:.3} ms"
    );
    let mut failed = false;
    if p50 > P50_BUDGET_MS {
        println!("FAIL: p50 {p50:.3} ms over the {P50_BUDGET_MS} ms budget");
        failed = true;
    }
    if p99 > P99_BUDGET_MS {
        println!("FAIL: p99 {p99:.3} ms over the {P99_BUDGET_MS} ms budget");
        failed = true;
    }
    if fact_bytes > MEMORY_BUDGET_BYTES {
        println!("FAIL: {fact_bytes} fact bytes over the {MEMORY_BUDGET_BYTES} byte budget");
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
}
