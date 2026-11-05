//! GeoSPARQL query functions (MEDULLA-FEDERATED-QUERY-R709.1, R709.2).
#![cfg(feature = "geosparql")]

use medulla_geo::{Geometry, parse_wkt, predicates};
use medulla_rdf::geosparql::{
    GEOF_FUNCTIONS, eval_buffer, eval_distance, eval_envelope, eval_relation,
    is_registered_function,
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
