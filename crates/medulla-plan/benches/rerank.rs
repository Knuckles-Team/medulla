//! Lexical rerank bench (MEDULLA-FEDERATED-QUERY-R652.6).
//!
//! One `rerank_top` scores 64 candidates of 256 tokens each with the lexical BM25 scorer on
//! CPU and calibrates them with a pinned curve. The gate keys `rerank_top64` in
//! `thresholds.json` (p50 6 ms, p99 25 ms).
//!
//! Run: `cargo bench -p medulla-plan --bench rerank`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::rerank::{rerank_top, CalibrationCurve, LexicalDoc, ScorerId};
use std::hint::black_box;

const CANDIDATES: usize = 64;
const TOKENS: usize = 256;
const VOCABULARY: usize = 512;

/// A deterministic 256-token text: token `t` of document `n` is `w{(n * 31 + t * 17) % 512}`.
fn text(n: usize) -> String {
    (0..TOKENS)
        .map(|t| format!("w{}", (n * 31 + t * 17) % VOCABULARY))
        .collect::<Vec<_>>()
        .join(" ")
}

fn bench_rerank(c: &mut Criterion) {
    let texts: Vec<String> = (0..CANDIDATES).map(text).collect();
    let ids: Vec<String> = (0..CANDIDATES).map(|n| format!("node-{n:03}")).collect();
    let docs: Vec<LexicalDoc<'_>> = ids
        .iter()
        .zip(&texts)
        .map(|(id, text)| LexicalDoc {
            node_id: id,
            name: "",
            description: "",
            text,
        })
        .collect();
    let query = "w3 w20 w37 w54 w71 w88";
    let curve = CalibrationCurve::new(
        ScorerId::new("lexical_bm25", "1").expect("a scorer id"),
        vec![(0.0, 0.0), (4.0, 0.5), (16.0, 1.0)],
        "0".repeat(64),
    )
    .expect("a calibration curve");
    c.bench_function("rerank_top64", |b| {
        b.iter(|| {
            let (hits, _) = rerank_top(black_box(query), black_box(&docs), CANDIDATES, Some(&curve));
            black_box(hits);
        });
    });
}

criterion_group!(benches, bench_rerank);
criterion_main!(benches);
