//! Context bundle compile bench (MEDULLA-FEDERATED-QUERY-R650.6).
//!
//! One `compile` fuses two sources of 256 ranked hits each and selects inside an
//! 8,000-token budget with the exact package solver. The gate keys
//! `context_bundle_compile` in `thresholds.json` (p50 2 ms, p99 12 ms).
//!
//! Run: `cargo bench -p medulla-plan --bench context_bundle`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_decision_types::decision::context_package::ContextPackageBudget;
use medulla_plan::context_bundle::{
    compile, Citation, CompileInputs, ContextBundleRequest, ContextKnobs, ContextMode,
    ResolutionSpan, SourceHit, SourceRanking,
};
use std::hint::black_box;

const HITS: usize = 256;

fn digest(n: usize) -> String {
    format!("{n:064x}")
}

fn hit(n: usize, score: f32) -> SourceHit {
    let spans = (0..2u8)
        .map(|resolution| ResolutionSpan {
            resolution,
            tokens: 40 + 80 * u64::from(resolution) + (n % 7) as u64,
            citation: Citation::local(
                format!("doc-{n}"),
                0,
                100 + u64::from(resolution),
                1,
                digest(n * 2 + usize::from(resolution)),
            ),
        })
        .collect();
    SourceHit {
        node_id: format!("node-{n:04}"),
        score,
        spans,
        hop_path: Vec::new(),
        derived_from: Vec::new(),
    }
}

fn ranking(source: &str, shift: usize) -> SourceRanking {
    SourceRanking {
        source: source.into(),
        curve: None,
        hits: (0..HITS)
            .map(|n| hit((n + shift) % HITS, 1.0 - n as f32 / HITS as f32))
            .collect(),
    }
}

fn bench_compile(c: &mut Criterion) {
    let request = ContextBundleRequest {
        query_text: "incident runbook".into(),
        query_embedding: Vec::new(),
        budget: ContextPackageBudget {
            window_tokens: 8_000,
            reserved_output_tokens: 0,
            caller_ceiling_tokens: None,
            money_ceiling_micro: None,
            price_micro_per_token: None,
            latency_ceiling_ms: None,
            tokens_per_second: None,
        },
        tokenizer_id: "bench".into(),
        tokenizer_version: "1".into(),
        mode: ContextMode::Hybrid,
        as_of: None,
        agent_ref: None,
        top_n: 256,
    };
    let inputs = CompileInputs {
        tenant: "bench".into(),
        graph_revision: 1,
        policy_digest: "p".into(),
        rankings: vec![ranking("lexical", 0), ranking("semantic", 3)],
        pre_excluded: Vec::new(),
        unchecked_dropped: Vec::new(),
        knobs: ContextKnobs::default(),
        rerank: None,
    };
    c.bench_function("context_bundle_compile", |b| {
        b.iter(|| {
            let bundle = compile(black_box(&request), black_box(&inputs)).expect("compiles");
            black_box(bundle);
        });
    });
}

criterion_group!(benches, bench_compile);
criterion_main!(benches);
