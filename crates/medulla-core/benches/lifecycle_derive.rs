//! Lifecycle edge derivation over a million-symbol graph (MEDULLA-SDLC-R001.6).
//!
//! Nightly bench group `lifecycle/derive`: `derive_lifecycle_edges` for a 50-path changeset
//! against a code graph view that holds 1,000,000 symbols. Budgets: p99 below 5 ms and peak
//! memory growth below 64 MiB. The run exits non-zero when a budget is exceeded. A service
//! level, never a pull request gate; the gate is the served 50-path changeset test.
//!
// spec: MEDULLA-SDLC-R001.6
//!
//! Run: cargo bench -p medulla-core --bench lifecycle_derive

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use medulla_core::lifecycle::derive::{
    derive_lifecycle_edges, symbols_by_path, CodeGraphView, Span, SymbolFact,
};
use medulla_source_types::source_changeset::{ChangedPath, Changeset, PathOp, Trailers};

const SYMBOLS: usize = 1_000_000;
const PATHS: usize = 50;
const RUNS: usize = 200;
const BUDGET_P99: Duration = Duration::from_millis(5);
const BUDGET_PEAK_BYTES: u64 = 64 * 1024 * 1024;

/// The peak resident set of this process in bytes (Linux `VmHWM`), 0 where unreadable.
fn peak_resident() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        })
        .map_or(0, |kib| kib * 1024)
}

fn main() {
    let path = |n: usize| format!("src/file_{n}.rs");
    // 20,000 symbols per path, 50 changed paths; the changed span covers 20 symbols of each.
    let per_path = SYMBOLS / PATHS;
    let symbols: Vec<SymbolFact> = (0..SYMBOLS)
        .map(|n| SymbolFact {
            id: format!("symbol:{n}"),
            path: path(n / per_path),
            span: Span {
                start: ((n % per_path) * 10) as u32,
                end: ((n % per_path) * 10 + 9) as u32,
            },
        })
        .collect();
    let changed_spans: BTreeMap<String, Vec<Span>> = (0..PATHS)
        .map(|p| (path(p), vec![Span { start: 0, end: 200 }]))
        .collect();
    let view = CodeGraphView {
        symbols: symbols_by_path(symbols),
        changed_spans,
        on_default_branch: true,
        ..CodeGraphView::default()
    };
    let changeset = Changeset {
        id: "bench-changeset".into(),
        repo: "bench".into(),
        parents: Vec::new(),
        author_principal: "bench".into(),
        message: "bench".into(),
        changed_paths: (0..PATHS)
            .map(|p| ChangedPath {
                path: path(p),
                op: PathOp::Modify,
                old_blob: None,
                new_blob: None,
                mode: 0o644,
            })
            .collect(),
        created_at: 1,
        trailers: Trailers {
            spec_ids: vec!["BENCH-R001".into()],
            fixes: Vec::new(),
            withdraw: Vec::new(),
            withdraw_reason: None,
        },
        agent_identity: None,
        run_trace_id: None,
        git: None,
        symbol_edit: None,
    };
    let before = peak_resident();
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let edges = derive_lifecycle_edges(&changeset, &view);
        samples.push(started.elapsed());
        // 20 changed symbols per path, each a `changes` and an `implements` edge, plus the
        // `citesRequirement` edge.
        assert_eq!(edges.len(), PATHS * 20 * 2 + 1);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let growth = peak_resident().saturating_sub(before);
    println!(
        "lifecycle/derive p99 = {p99:?} (budget {BUDGET_P99:?}), peak growth = {growth} bytes (budget {BUDGET_PEAK_BYTES})"
    );
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        growth < BUDGET_PEAK_BYTES,
        "peak growth {growth} exceeds {BUDGET_PEAK_BYTES}"
    );
}
