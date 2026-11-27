//! `code_health::sweep` over a 100,000-symbol code index (MEDULLA-REPO-INGEST-R700.6).
//!
//! Criterion group `code_health`: the reachability sweep `Method::CodeHealthSweep` runs after
//! `handle_code_health_sweep` projects the code index into a `CodeGraph`. The graph has one
//! reachable chain from the declared entry point and a dead half, so the sweep both walks and
//! reports. Budgets: p50 at most 30 ms, p99 at most 200 ms, peak memory at most 128 MiB per
//! request. The sweep is a read and writes nothing. Nightly only; never a PR gate.
//!
//! Run: `cargo bench -p medulla-h-query --bench code_health`

use std::collections::{BTreeMap, BTreeSet};

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_ingestion_types::code_health::{sweep, CodeGraph};

const SYMBOLS: usize = 100_000;
const STEP_BUDGET: u64 = 1_000_000;

/// The first half is a call chain from `s0`; the second half is referenced by nothing.
fn code_index(entry: bool) -> CodeGraph {
    let symbols: BTreeSet<String> = (0..SYMBOLS).map(|i| format!("s{i}")).collect();
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for i in 0..SYMBOLS / 2 - 1 {
        references.insert(format!("s{i}"), BTreeSet::from([format!("s{}", i + 1)]));
    }
    CodeGraph {
        repo: "bench-repo".into(),
        symbols,
        references,
        entry_points: if entry {
            BTreeSet::from(["s0".to_string()])
        } else {
            BTreeSet::new()
        },
    }
}

// spec: MEDULLA-REPO-INGEST-R700.6
fn code_health(c: &mut Criterion) {
    let with_entry = code_index(true);
    let without_entry = code_index(false);
    let mut group = c.benchmark_group("code_health");
    group.bench_function("sweep_100k_symbols", |b| {
        b.iter(|| {
            let findings = sweep(black_box(&with_entry), STEP_BUDGET);
            assert_eq!(findings.len(), SYMBOLS / 2);
            findings
        })
    });
    group.bench_function("sweep_100k_symbols_no_entry_point", |b| {
        b.iter(|| {
            let findings = sweep(black_box(&without_entry), STEP_BUDGET);
            assert_eq!(findings.len(), SYMBOLS);
            findings
        })
    });
    group.finish();
}

criterion_group!(benches, code_health);
criterion_main!(benches);
