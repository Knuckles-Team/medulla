//! GeoSPARQL query functions (MEDULLA-FEDERATED-QUERY-R709.1, R709.2).
#![cfg(feature = "geosparql")]

use medulla_geo::{parse_wkt, predicates, Geometry};
use medulla_rdf::geosparql::{
    eval_buffer, eval_distance, eval_envelope, eval_relation, is_registered_function,
    GEOF_FUNCTIONS,
};

/// MEDULLA-FEDERATED-QUERY-R709.1: the registry lists exactly the 11 GeoSPARQL query
/// functions, and each one evaluates.
// spec: MEDULLA-FEDERATED-QUERY-R709.1
#[test]
fn functions_registered() {
    let expected = [
        "sfEquals",
        "sfDisjoint",
        "sfIntersects",
        "sfTouches",
        "sfWithin",
        "sfContains",
        "sfOverlaps",
        "sfCrosses",
        "distance",
        "buffer",
        "envelope",
    ];
    assert_eq!(GEOF_FUNCTIONS, expected);
    for name in expected {
        assert!(is_registered_function(name), "{name} registered");
    }
    assert!(!is_registered_function("sfNope"));

    let poly = "POLYGON((0 0, 4 0, 4 4, 0 4, 0 0))";
    for name in &expected[..8] {
        assert!(
            eval_relation(name, "POINT(1 1)", poly).is_some(),
            "{name} evaluates"
        );
    }
    assert_eq!(eval_distance("POINT(0 0)", "POINT(3 4)", ""), Some(5.0));
    assert!(eval_buffer("POINT(0 0)", 1.0, "").is_some());
    let env = eval_envelope("LINESTRING(1 2, 3 5)").expect("envelope");
    assert!(parse_wkt(&env).is_ok(), "envelope WKT re-parses: {env}");
    assert_eq!(env, "POLYGON((1 2, 3 2, 3 5, 1 5, 1 2))");
}

/// MEDULLA-FEDERATED-QUERY-R709.2: every Simple-Features relation returns exactly what the
/// `medulla-geo` kernel predicate returns, including shared-boundary pairs.
// spec: MEDULLA-FEDERATED-QUERY-R709.2
#[test]
fn topology_matches_kernel() {
    type Pred = fn(&Geometry, &Geometry) -> bool;
    let kernel: [(&str, Pred); 8] = [
        ("sfEquals", predicates::equals),
        ("sfDisjoint", predicates::disjoint),
        ("sfIntersects", predicates::intersects),
        ("sfTouches", predicates::touches),
        ("sfWithin", predicates::within),
        ("sfContains", predicates::contains),
        ("sfOverlaps", predicates::overlaps),
        ("sfCrosses", predicates::crosses),
    ];
    let shapes = [
        "POINT(1 1)",
        "POINT(4 2)",
        "POINT(9 9)",
        "LINESTRING(-1 2, 5 2)",
        "LINESTRING(4 0, 4 4)",
        "POLYGON((0 0, 4 0, 4 4, 0 4, 0 0))",
        "POLYGON((4 0, 8 0, 8 4, 4 4, 4 0))",
        "POLYGON((2 2, 6 2, 6 6, 2 6, 2 2))",
        "POLYGON((1 1, 2 1, 2 2, 1 2, 1 1))",
    ];
    for a in shapes {
        for b in shapes {
            let ga = parse_wkt(a).expect("wkt a");
            let gb = parse_wkt(b).expect("wkt b");
            for (name, pred) in kernel {
                assert_eq!(
                    eval_relation(name, a, b),
                    Some(pred(&ga, &gb)),
                    "{name}({a}, {b}) matches the kernel"
                );
            }
        }
    }
}

/// MEDULLA-FEDERATED-QUERY-R709.7: the R-tree path and the scan path return the same rows for
/// every spatial function over 1,000 fixtures, after inserts and after deletes.
// spec: MEDULLA-FEDERATED-QUERY-R709.7
#[test]
fn indexed_and_scanned_results_are_equal() {
    use medulla_rdf::spatial_index::{eval_relation_over, SpatialIndex, TermId};
    use std::collections::BTreeMap;

    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 10_000) as f64 / 100.0
    };
    let mut literals: BTreeMap<TermId, String> = BTreeMap::new();
    for i in 0..1_000u64 {
        let (x, y) = (next(), next());
        let wkt = match i % 3 {
            0 => format!("POINT({x} {y})"),
            1 => format!("LINESTRING({x} {y}, {} {})", x + 3.0, y + 1.5),
            _ => format!(
                "POLYGON(({x} {y}, {} {y}, {} {}, {x} {}, {x} {y}))",
                x + 4.0,
                x + 4.0,
                y + 4.0,
                y + 4.0
            ),
        };
        literals.insert(TermId(i), wkt);
    }
    let constants = [
        "POLYGON((20 20, 60 20, 60 60, 20 60, 20 20))",
        "POINT(50 50)",
        "LINESTRING(0 0, 100 100)",
        "POLYGON((0 0, 100 0, 100 100, 0 100, 0 0))",
    ];
    let functions = [
        ("sfWithin", None),
        ("sfIntersects", None),
        ("sfContains", None),
        ("sfOverlaps", None),
        ("sfEquals", None),
        ("sfTouches", None),
        ("sfCrosses", None),
        ("sfDisjoint", None),
        ("distance", Some(5.0)),
    ];
    let compare = |index: &SpatialIndex, literals: &BTreeMap<TermId, String>| {
        let all: Vec<(TermId, String)> = literals.iter().map(|(t, s)| (*t, s.clone())).collect();
        for constant in constants {
            for (name, bound) in functions {
                let scanned = eval_relation_over(name, bound, constant, &all, None)
                    .expect("scan")
                    .expect("known function");
                let indexed = eval_relation_over(name, bound, constant, &all, Some(index))
                    .expect("indexed")
                    .expect("known function");
                assert_eq!(indexed, scanned, "{name}({constant}) agrees on both paths");
            }
        }
    };

    let mut index = SpatialIndex::new("urn:graph:fixture");
    let inserts: Vec<(TermId, &str)> = literals.iter().map(|(t, s)| (*t, s.as_str())).collect();
    index.apply(&inserts, &[]).expect("insert");
    assert_eq!(index.len(), 1_000);
    compare(&index, &literals);

    let deleted: Vec<TermId> = (0..1_000u64).step_by(4).map(TermId).collect();
    for t in &deleted {
        literals.remove(t);
    }
    index.apply(&[], &deleted).expect("delete");
    assert_eq!(index.len(), 750);
    compare(&index, &literals);

    let refused = index.apply(&[(TermId(5_000), "NOTGEOM(1 2)")], &[]);
    assert!(refused.is_err(), "an invalid literal refuses the write");
    assert_eq!(
        index.len(),
        750,
        "a refused write leaves the index unchanged"
    );
}
