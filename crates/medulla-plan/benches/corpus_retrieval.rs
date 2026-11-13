//! Corpus grep bench (MEDULLA-FEDERATED-QUERY-R653.6).
//!
//! One `grep_corpus` call scans a 1,000,000-token corpus (2,000 nodes of 500 tokens,
//! 50 tokens a line) for a pattern whose only matches sit in the last nodes, so the
//! scan reads every node before it fills its limit. The gate key `corpus_grep_1m` in
//! `thresholds.json` caps p50 at 4 ms and p99 at 20 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench corpus_retrieval`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::corpus_retrieval::{grep_corpus, TextNode};
use std::hint::black_box;

const NODES: usize = 2_000;
const TOKENS_PER_NODE: usize = 500;
const TOKENS_PER_LINE: usize = 50;
const LIMIT: usize = 20;

fn node(n: usize) -> TextNode {
    let mut body = String::with_capacity(TOKENS_PER_NODE * 7);
    for token in 0..TOKENS_PER_NODE {
        if token > 0 {
            body.push(if token % TOKENS_PER_LINE == 0 {
                '\n'
            } else {
                ' '
            });
        }
        if n >= NODES - 10 && token == 7 {
            body.push_str("needle_marker");
        } else {
            body.push_str(&format!("w{}", (n * 31 + token) % 9973));
        }
    }
    TextNode {
        node_id: format!("node-{n:05}"),
        fields: vec![("body".to_string(), body)],
    }
}

fn bench_grep(c: &mut Criterion) {
    let corpus: Vec<TextNode> = (0..NODES).map(node).collect();
    c.bench_function("corpus_grep_1m", |b| {
        b.iter(|| {
            let found = grep_corpus(black_box(r"needle_\w+"), black_box(&corpus), LIMIT)
                .expect("pattern compiles");
            assert_eq!(found.len(), 10);
            black_box(found);
        });
    });
}

criterion_group!(benches, bench_grep);
criterion_main!(benches);
