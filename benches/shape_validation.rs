//! Compiled shape memoization against parsing per write (MEDULLA-DURABLE-KERNEL-R064).
//! 1 000 writes are validated against a 50-shape graph in both variants; one receipt
//! with both variants, hardware and profile goes to
//! `benches/receipts/<release>/shape_validation.json` through
//! `benches/common/claim.rs::write_receipt`. The memoized path is adopted only when
//! its p50 is at least 20 percent lower; without that win the bench exits non-zero so
//! the memoized path is deleted.

#[path = "common/claim.rs"]
mod claim;
#[path = "common/latency_stats.rs"]
mod latency_stats;
#[path = "common/receipt_fields.rs"]
mod receipt_fields;
#[path = "common/shape_receipt.rs"]
mod shape_receipt;

use std::time::Instant;

use medulla_rdf::oxrdf::Graph;
use medulla_shacl::{graph_from_turtle, validate, CompiledShapes};
use shape_receipt::{write_shape_receipt, ShapeBenchCase};

const SHAPE_COUNT: usize = 50;
const WRITES: usize = 1_000;

fn shapes_turtle() -> String {
    let mut ttl = String::from(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <https://example.org/s#> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
    );
    for i in 0..SHAPE_COUNT {
        ttl.push_str(&format!(
            "ex:Shape{i} a sh:NodeShape ; sh:targetClass ex:Class{i} ; \
             sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ; \
             sh:maxLength 64 ] .\n"
        ));
    }
    ttl
}

fn write_graph(i: usize) -> Graph {
    let ttl = format!(
        "@prefix ex: <https://example.org/s#> .\nex:n{i} a ex:Class{} ; ex:name \"node {i}\" .\n",
        i % SHAPE_COUNT
    );
    graph_from_turtle(&ttl).expect("write graph parses")
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn main() {
    let shapes = graph_from_turtle(&shapes_turtle()).expect("shapes parse");
    let writes: Vec<Graph> = (0..WRITES).map(write_graph).collect();

    let mut parse_per_write_ms = Vec::with_capacity(WRITES);
    for data in &writes {
        let start = Instant::now();
        let report = validate(&shapes, data).expect("validate");
        parse_per_write_ms.push(elapsed_ms(start));
        assert!(report.conforms);
    }

    let compiled = CompiledShapes::compile(&shapes);
    let mut memoized_ms = Vec::with_capacity(WRITES);
    for data in &writes {
        let start = Instant::now();
        let report = compiled.validate(data).expect("validate");
        memoized_ms.push(elapsed_ms(start));
        assert!(report.conforms);
    }

    let case = ShapeBenchCase {
        writes: WRITES,
        shapes: SHAPE_COUNT,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        memoized_ms,
        parse_per_write_ms,
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/receipts");
    write_shape_receipt(&root, env!("CARGO_PKG_VERSION"), &case).expect("write receipt");
    if !case.adopt_memoized() {
        eprintln!(
            "memoized shapes are not at least 20 percent faster at p50 (win {:.3}): delete the memoized path",
            case.p50_win()
        );
        std::process::exit(1);
    }
}
