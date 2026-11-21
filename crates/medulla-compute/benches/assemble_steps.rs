//! Assembly step bench (MEDULLA-ORCH-VOCAB-R007.1): the requirement derivation
//! over the largest admitted step set, 64 steps of 16 capabilities each.
//!
//! Run: cargo bench -p medulla-compute --bench assemble_steps

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_decision_outcome_types::decision::derivation::required_capabilities;
use medulla_decision_types::decision::{AssemblyRequirements, AssemblyStep};
use medulla_types::contract::BoundedVec;
use std::hint::black_box;

const ONTOLOGY: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn steps() -> Vec<AssemblyStep> {
    (0..64u16)
        .map(|n| AssemblyStep {
            task: "eg:task/research".to_string(),
            step: format!("urn:step:{n:02}"),
            position: n,
            capabilities: BoundedVec::new(
                (0..16)
                    .map(|c| format!("eg:capability/bench/{n:02}-{c:02}"))
                    .collect(),
            )
            .expect("sixteen capabilities fit"),
        })
        .collect()
}

fn assemble_steps(c: &mut Criterion) {
    let steps = steps();
    let requirements = AssemblyRequirements::default();
    let mut group = c.benchmark_group("assemble/steps");
    group.bench_function("derive_64_steps", |b| {
        b.iter(|| {
            black_box(required_capabilities(
                black_box(&requirements),
                black_box(&steps),
                ONTOLOGY,
            ))
        })
    });
    group.finish();
}

criterion_group!(benches, assemble_steps);
criterion_main!(benches);
