//! MEDULLA-DURABLE-KERNEL-R064: the memoized compiled shapes report exactly what
//! parse-per-write validation reports.

use medulla_shacl::{graph_from_turtle, validate, CompiledShapes};

const SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://example.org/s#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ] .
ex:OffShape a sh:NodeShape ; sh:deactivated true ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:age ; sh:minCount 1 ] .
"#;

const CONFORMING: &str = r#"
@prefix ex: <https://example.org/s#> .
ex:a a ex:Person ; ex:name "Ada" .
"#;

const VIOLATING: &str = r#"
@prefix ex: <https://example.org/s#> .
ex:b a ex:Person .
"#;

// spec: MEDULLA-DURABLE-KERNEL-R064
#[test]
fn compiled_shapes_report_the_same_as_parse_per_write() {
    let shapes = graph_from_turtle(SHAPES).unwrap();
    let compiled = CompiledShapes::compile(&shapes);
    assert_eq!(
        compiled.root_count(),
        1,
        "the deactivated shape is not held"
    );
    for data_ttl in [CONFORMING, VIOLATING] {
        let data = graph_from_turtle(data_ttl).unwrap();
        let direct = validate(&shapes, &data).unwrap();
        let memoized = compiled.validate(&data).unwrap();
        assert_eq!(direct.conforms, memoized.conforms);
        assert_eq!(direct.results.len(), memoized.results.len());
    }
    let bad = graph_from_turtle(VIOLATING).unwrap();
    assert!(!compiled.validate(&bad).unwrap().conforms);
    let good = graph_from_turtle(CONFORMING).unwrap();
    assert!(compiled.validate(&good).unwrap().conforms);
}
