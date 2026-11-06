//! Ontology edit chain append and revert (MEDULLA-TYPED-PACKS-R651.6).
//!
//! SLO: appending plus reverting 100 edits over a chain of 10,000 stays under
//! 5 ms at p99. The bench times the chain rules, the part of the commit that
//! grows with the chain; the node writes are O(1) per edit.
//!
//! Run: `cargo bench --bench ontology_edit_revert`. Nightly suite.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_core::ontology::edits::{append, revert, EditDraft};
use medulla_types::contract::Digest256;
use medulla_types::ontology_edit::{EditKind, OntologyEdit};
use std::hint::black_box;

const CHAIN: usize = 10_000;
const REVERTED: usize = 100;

fn digest(seed: usize) -> Option<Digest256> {
    Some(Digest256::sha256(seed.to_string().as_bytes()))
}

fn push(chain: &mut Vec<OntologyEdit>, subject: &str, step: usize) {
    let edit = EditDraft {
        kind: EditKind::Change,
        subject_iri: subject.to_string(),
        before_digest: digest(step),
        after_digest: digest(step + 1),
        sealed_by: None,
    }
    .link(chain);
    append(chain, edit).expect("a linked edit appends");
}

fn base_chain() -> Vec<OntologyEdit> {
    let mut chain = Vec::with_capacity(CHAIN + REVERTED);
    for step in 0..CHAIN - REVERTED {
        push(&mut chain, "http://example.org/other", step);
    }
    chain
}

fn bench_append_and_revert(c: &mut Criterion) {
    let base = base_chain();
    c.bench_function("append_100_and_revert_over_10000", |bench| {
        bench.iter(|| {
            let mut chain = base.clone();
            for step in 0..REVERTED {
                push(&mut chain, "http://example.org/subject", step);
            }
            let appended = revert(&mut chain, "http://example.org/subject", None);
            black_box(appended).expect("revert");
        });
    });
}

criterion_group!(benches, bench_append_and_revert);
criterion_main!(benches);
