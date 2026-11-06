//! MEDULLA-UNIFIED-DATA-PLANE-R750.753 hot-path receipt: group `history_search`
//! (per MEDULLA-DECISION-ENGINE-R178).
//!
//! Budget (spec): p50 12 ms, p99 35 ms, peak memory 128 MiB. The dataset is
//! one million `Step` nodes over two scopes of one tenant, each step's text in
//! the lexical index. One request runs the `HistorySearch` plan
//! (`Scan Step` → tenant and scope filter → `RankText` → `Limit k`) through
//! the unified executor, the plan the served handler builds. Prints p50/p99
//! beside the process RSS growth across the measured loop. Benchmarks never
//! gate a PR.
//!
//! Deterministic: fixed texts, no RNG.
//!
//! Run: cargo bench --features text,obs --bench history_search

#[path = "common/latency.rs"]
mod latency;

use std::time::Instant;

use medulla_exec::algebra::{Op, Plan, Pred};
use medulla_exec::exec::PlanCtx;
use medulla_graph_core::graph::GraphCore;
use medulla_semantic_index::semantic::SemanticStore;
use medulla_text::TextIndex;

const STEPS: usize = 1_000_000;
const WARMUP: usize = 20;
const ITERATIONS: usize = 200;
const WORDS: [&str; 8] = [
    "deploy", "gateway", "release", "wheel", "schema", "replica", "ledger", "budget",
];

fn step_text(i: usize) -> String {
    format!("{} the {} step {i}", WORDS[i % 8], WORDS[(i / 8) % 8])
}

fn dataset() -> (GraphCore, TextIndex) {
    let core = GraphCore::new();
    let mut text = TextIndex::in_memory().expect("in-memory text index");
    for i in 0..STEPS {
        let id = format!("step:{i}");
        let scope = if i % 2 == 0 {
            "project:alpha"
        } else {
            "project:beta"
        };
        let props = serde_json::json!({
            "node_type": "Step", "tenant": "acme", "scope": scope,
            "run_id": format!("run-{}", i / 16), "index": i % 16, "text": step_text(i),
        });
        core.add_node(
            id.clone(),
            rmp_serde::to_vec_named(&props).expect("encode step"),
        );
        text.upsert(&id, &step_text(i));
    }
    text.commit().expect("commit text index");
    (core, text)
}

fn plan(query: &str) -> Plan {
    let eq = |prop: &str, value: &str| Pred::Eq {
        prop: prop.into(),
        value: value.into(),
    };
    Plan::new(vec![
        Op::Scan {
            label: "Step".into(),
        },
        Op::Filter {
            preds: vec![eq("tenant", "acme"), eq("scope", "project:alpha")],
        },
        Op::RankText {
            query: query.into(),
        },
        Op::Limit { k: 10 },
    ])
}

fn main() {
    let (core, text) = dataset();
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic).with_text(&text);
    let request = |i: usize| {
        let rows = medulla_exec::exec::execute(&plan(WORDS[i % 8]), &ctx).expect("plan runs");
        rows.rows().len()
    };
    for i in 0..WARMUP {
        assert!(request(i) <= 10);
    }
    let rss_before = latency::rss_mib();
    let mut samples = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS {
        let start = Instant::now();
        let hits = request(i);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert!(hits <= 10);
    }
    let rss_growth = latency::rss_mib() - rss_before;
    samples.sort_by(f64::total_cmp);
    println!(
        "history_search: p50 {:.4} ms (budget 12), p99 {:.4} ms (budget 35), rss growth {:.2} MiB (budget 128)",
        latency::percentile(&samples, 0.50),
        latency::percentile(&samples, 0.99),
        rss_growth
    );
}
