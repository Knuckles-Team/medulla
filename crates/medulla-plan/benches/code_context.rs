//! Code-context impact bench (MEDULLA-FEDERATED-QUERY-R654.6).
//!
//! One `code_context` call asks the impact question at depth 3 over a symbol index of
//! 1,000,000 symbols. The served `CodeContext` handler calls exactly this function off-lock
//! after it has read the caller's projected core into a `CodeIndex`. The gate key
//! `code_context_impact` in `thresholds.json` caps p50 at 3 ms and p99 at 15 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench code_context`.
// spec: MEDULLA-FEDERATED-QUERY-R654.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::code_context::{code_context, CodeEdge, CodeIndex};
use medulla_types::code_context::{CodeContextRequest, CodeQuestion};
use std::hint::black_box;

const SYMBOLS: usize = 1_000_000;
/// Every symbol calls the symbol this many places ahead, so the graph is not a bare chain.
const FAN: usize = 3;

fn name(n: usize) -> String {
    format!("repo/{:04}.rs::sym{n:07}", n / 1_000)
}

fn index() -> CodeIndex {
    let symbols: Vec<String> = (0..SYMBOLS).map(name).collect();
    let mut edges = Vec::with_capacity(SYMBOLS * 2);
    for n in 0..SYMBOLS - 1 {
        edges.push(CodeEdge::new(name(n), name(n + 1)));
        if n + FAN < SYMBOLS {
            edges.push(CodeEdge::new(name(n), name(n + FAN)));
        }
    }
    CodeIndex {
        symbols,
        edges,
        missing_repositories: Vec::new(),
    }
}

fn bench_impact(c: &mut Criterion) {
    let index = index();
    let request = CodeContextRequest {
        anchor: name(SYMBOLS / 2),
        question: CodeQuestion::Impact,
        depth: Some(3),
        limit: None,
    };
    c.bench_function("code_context_impact", |b| {
        b.iter(|| {
            let answer = code_context(black_box(&index), black_box(&request))
                .expect("the anchor names one symbol");
            black_box(answer.impact.len());
        });
    });
}

criterion_group!(benches, bench_impact);
criterion_main!(benches);
