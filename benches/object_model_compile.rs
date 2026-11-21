//! MEDULLA-TYPED-PACKS-R650.6 hot-path receipt: compiling a pack's object-model
//! declarations to SHACL triples.
//!
//! Budget (spec): `compile_shapes` p99 under 20 ms for 1,000 declarations. The
//! compiler is a pure function, linear in the declarations, so this bench builds
//! one deterministic model of 1,000 declarations (value types with the properties
//! that use them, capped and junction links, interfaces and their implementations)
//! and prints p50/p99 over repeated compiles.
//!
//! Deterministic: fixed names, no RNG.
//!
//! Run: cargo bench --bench object_model_compile

use std::time::Instant;

use medulla_core::ontology::object_model::{
    compile_shapes, Implementation, InterfaceDecl, LinkType, ObjectModel, PropertyDecl, ValueType,
};

const DECLARATIONS: usize = 1_000;
const ITERATIONS: usize = 200;
const WARMUP: usize = 20;
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const BASE: &str = "https://example.org/bench";

/// 1,000 declarations: value types, each with a property that uses it, link
/// types (every tenth a junction), and interfaces with one implementation each.
fn model() -> ObjectModel {
    let mut model = ObjectModel::default();
    for index in 0..DECLARATIONS {
        match index % 4 {
            0 => {
                let name = format!("{BASE}#ValueType{index}");
                model.value_types.push(ValueType {
                    name: name.clone(),
                    base: XSD_STRING.to_string(),
                    pattern: Some("^[A-Z]{3}-[0-9]+$".to_string()),
                    min: None,
                    max: None,
                    max_len: Some(64),
                    one_of: vec!["ABC-1".to_string(), "ABC-2".to_string()],
                });
                model.properties.push(PropertyDecl {
                    class: format!("{BASE}#Class{}", index % 50),
                    property: format!("{BASE}#property{index}"),
                    value_type: Some(name),
                });
            }
            1 | 2 => model.link_types.push(LinkType {
                name: format!("{BASE}#link{index}"),
                from_class: format!("{BASE}#Class{}", index % 50),
                to_class: format!("{BASE}#Class{}", (index + 1) % 50),
                min_card: 0,
                max_card: std::num::NonZeroU32::new(3),
                junction: (index % 10 == 1).then(|| format!("{BASE}#Junction{index}")),
            }),
            _ => {
                let interface = format!("{BASE}#Interface{index}");
                model.interfaces.push(InterfaceDecl {
                    name: interface.clone(),
                    required_properties: vec![format!("{BASE}#property{}", index - 3)],
                    required_links: vec![format!("{BASE}#link{}", index - 2)],
                });
                model.implementations.push(Implementation {
                    class: format!("{BASE}#Class{}", index % 50),
                    interface,
                });
            }
        }
    }
    model
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

fn main() {
    let model = model();
    let declared = model.value_types.len()
        + model.link_types.len()
        + model.interfaces.len()
        + model.properties.len()
        + model.implementations.len();
    // Pin the work the bench measures: a compile that emits nothing proves nothing.
    let triples = compile_shapes(&model).len();
    assert!(triples > declared, "compile emitted {triples} triples");

    for _ in 0..WARMUP {
        let _ = compile_shapes(&model).len();
    }
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let compiled = compile_shapes(&model);
        samples.push(start.elapsed().as_secs_f64() * 1e3);
        assert_eq!(compiled.len(), triples, "compile is deterministic");
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    let p50 = percentile(&sorted, 0.50);
    let p99 = percentile(&sorted, 0.99);
    println!("object_model_compile: declarations={declared} triples={triples} iters={ITERATIONS}");
    println!("  p50={p50:.4} ms  p99={p99:.4} ms");
    println!(
        "  budget p99 < 20 ms: {}",
        if p99 < 20.0 { "PASS" } else { "FAIL" }
    );
}
