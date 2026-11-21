//! Rule fixpoint bench (MEDULLA-FEDERATED-QUERY-R608.6).
//!
//! One `reason_facts` call derives the fixpoint of 200 rules over 1,000,000 ground facts:
//! rule `r<i>` is `p<i>(?x,?y) -> q<i>(?x,?y)`, and each of the 200 base predicates holds
//! 5,000 facts, so the fixpoint derives 1,000,000 facts. The gate key `rule_fixpoint_1m` in
//! `thresholds.json` caps p50 at 2 s and p99 at 6 s.
//!
//! Run: `cargo bench -p medulla-reasoner --bench rule_fixpoint`.
// spec: MEDULLA-FEDERATED-QUERY-R608.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_reasoner::owl::Ontology;
use medulla_reasoner::rules::{reason_facts, RuleSet};
use std::hint::black_box;

const RULES: usize = 200;
const FACTS_PER_RULE: usize = 5_000;

fn rules() -> RuleSet {
    let mut set = RuleSet::new();
    for i in 0..RULES {
        set.add_str(&format!("r{i}: p{i}(?x,?y) -> q{i}(?x,?y)"))
            .expect("the bench rule parses");
    }
    set
}

fn facts() -> Vec<(String, Vec<String>, f64)> {
    (0..RULES)
        .flat_map(|i| {
            (0..FACTS_PER_RULE).map(move |n| {
                (
                    format!("<http://example.org/p{i}>"),
                    vec![format!("<http://example.org/a{n}>"), format!("<http://example.org/b{n}>")],
                    1.0,
                )
            })
        })
        .collect()
}

fn bench_fixpoint(c: &mut Criterion) {
    let ontology = Ontology::default();
    let rules = rules();
    let facts = facts();
    assert_eq!(facts.len(), 1_000_000);
    let warm = reason_facts(&ontology, &facts, &rules);
    assert!(warm.complete, "the fixpoint converges within the default step bound");
    assert_eq!(warm.derived.len(), RULES * FACTS_PER_RULE);
    c.bench_function("rule_fixpoint_1m", |b| {
        b.iter(|| black_box(reason_facts(&ontology, black_box(&facts), &rules).derived.len()));
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_fixpoint
}
criterion_main!(benches);
