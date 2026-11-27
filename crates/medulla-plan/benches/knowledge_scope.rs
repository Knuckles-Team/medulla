//! Knowledge-scope bench (MEDULLA-FEDERATED-QUERY-R704.6).
//!
//! A scope of 50 container references is applied to the first `retrieval.scope_candidates`
//! (1024) fused candidates of a hybrid search, through `ResolvedScope::top_k`, the call
//! `Discover` makes. The scope never reads the corpus: the cost is bounded by the candidate
//! window, and the 50 references are held in a set built once per request. The gate key
//! `knowledge_scope_50` in `thresholds.json` caps p99 at 2 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench knowledge_scope`.
// spec: MEDULLA-FEDERATED-QUERY-R704.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_graph_core::graph::GraphCore;
use medulla_retrieval::knowledge_scope::ResolvedScope;
use medulla_work_types::orchestration::knowledge_scope::KnowledgeScope;
use serde_json::{json, Value};
use std::hint::black_box;

const WINDOW: usize = 1024;
const CONTAINERS: usize = 100;
const SCOPED_CONTAINERS: usize = 50;

fn blob(value: &Value) -> Vec<u8> {
    rmp_serde::to_vec_named(value).expect("encodes")
}

fn corpus() -> (GraphCore, Vec<(String, f32)>) {
    let core = GraphCore::new();
    for container in 0..CONTAINERS {
        core.add_node(
            format!("c{container}"),
            blob(&json!({"type": "SourceContainer"})),
        );
    }
    let mut scored = Vec::with_capacity(WINDOW);
    for n in 0..WINDOW {
        let id = format!("doc{n}");
        core.add_node(id.clone(), blob(&json!({"source_kind": "confluence"})));
        core.add_edge(
            id.clone(),
            format!("c{}", n % CONTAINERS),
            blob(&json!({"relationship": "inContainer"})),
        )
        .expect("edge");
        scored.push((id, 1.0 - n as f32 / WINDOW as f32));
    }
    (core, scored)
}

fn bench_scope(c: &mut Criterion) {
    let (core, scored) = corpus();
    let scope = KnowledgeScope {
        agent_ref: "agent:bench".into(),
        container_refs: (0..SCOPED_CONTAINERS).map(|n| format!("c{n}")).collect(),
        source_kinds: vec!["confluence".into()],
        ..KnowledgeScope::default()
    };
    c.bench_function("knowledge_scope_50", |b| {
        b.iter(|| {
            let resolved = ResolvedScope::resolve(&core, &scope);
            let kept = resolved.top_k(&core, scored.clone(), WINDOW);
            black_box(kept.len());
        });
    });
}

criterion_group!(benches, bench_scope);
criterion_main!(benches);
