//! Nightly bench: retracting one subclass edge from a 100,000-class ontology costs at
//! most 10 percent of a fresh classification.

use std::time::Instant;

use medulla_reasoner::owl::Reasoner;
use oxrdf::{NamedNode, Triple};

const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
/// Spine length and leaves per spine class: 1,000 x 100 = 100,000 classes.
const SPINE: usize = 1_000;
const LEAVES: usize = 99;
const BUDGET_PERCENT: f64 = 10.0;

fn edge(sub: &str, sup: &str) -> Triple {
    let node = |name: &str| NamedNode::new_unchecked(format!("http://example.org/{name}"));
    Triple::new(node(sub), NamedNode::new_unchecked(SUBCLASS_OF), node(sup))
}

fn hierarchy() -> Vec<Triple> {
    let spine = (1..SPINE).map(|i| edge(&format!("S{}", i - 1), &format!("S{i}")));
    let leaves = (0..SPINE)
        .flat_map(|i| (0..LEAVES).map(move |j| edge(&format!("L{i}_{j}"), &format!("S{i}"))));
    spine.chain(leaves).collect()
}

fn main() {
    let triples = hierarchy();
    let started = Instant::now();
    let mut reasoner = Reasoner::from_triples(&triples);
    reasoner.classify();
    let fresh = started.elapsed();

    let middle = vec![edge(
        &format!("S{}", SPINE / 2 - 1),
        &format!("S{}", SPINE / 2),
    )];
    let started = Instant::now();
    reasoner.retract_axioms(&middle);
    let retract = started.elapsed();

    let percent = 100.0 * retract.as_secs_f64() / fresh.as_secs_f64();
    println!("fresh {fresh:?}, retract {retract:?} ({percent:.2} percent)");
    assert!(
        percent <= BUDGET_PERCENT,
        "retraction took {percent:.2} percent of a fresh classification"
    );
}
