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

/// A test accessor over one snapshot: the boxes of every `geo:asWKT` literal in the
/// view, the way the maintained graph index exposes them through `SpatialAccessor`.
#[cfg(feature = "geosparql")]
struct ViewAccessor {
    ids: Vec<String>,
    tree: medulla_geo::rtree::RTree,
    version: u64,
}

#[cfg(feature = "geosparql")]
impl ViewAccessor {
    fn over(view: &medulla_graph_core::graph::GraphView, version: u64) -> Self {
        let mut ids = Vec::new();
        let mut boxes = Vec::new();
        for (id, blob) in &view.node_properties {
            let value = medulla_types::msgpack::decode_property_value(blob.as_slice()).unwrap();
            let cell = value.get(AS_WKT);
            let wkt = cell
                .and_then(|c| c.get("value").or(Some(c)))
                .and_then(|v| v.as_str());
            if let Some(bbox) = wkt.and_then(|w| parse_wkt(w).ok()).and_then(|g| g.bbox()) {
                boxes.push((ids.len(), bbox));
                ids.push(id.clone());
            }
        }
        Self {
            ids,
            tree: medulla_geo::rtree::RTree::build(&boxes),
            version,
        }
    }
}

#[cfg(feature = "geosparql")]
impl medulla_rdf::spatial_index::SpatialAccessor for ViewAccessor {
    fn version(&self) -> u64 {
        self.version
    }
    fn candidates(&self, window: &medulla_geo::Bbox) -> Vec<String> {
        let hits = self.tree.query_bbox(window).into_iter();
        hits.map(|pos| self.ids[pos].clone()).collect()
    }
}

#[cfg(feature = "geosparql")]
const AS_WKT: &str = "http://www.opengis.net/ont/geosparql#asWKT";

/// 1,000 features (points, lines, squares) as Turtle, skipping the ids in `deleted`.
#[cfg(feature = "geosparql")]
fn fixtures(deleted: &std::collections::BTreeSet<u64>) -> String {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 10_000) as f64 / 100.0
    };
    let mut ttl = String::new();
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
        if !deleted.contains(&i) {
            ttl.push_str(&format!(
                "<urn:f:{i}> <{AS_WKT}> \"{wkt}\"^^<http://www.opengis.net/ont/geosparql#wktLiteral> .\n"
            ));
        }
    }
    ttl
}

#[cfg(feature = "geosparql")]
fn view_of(ttl: &str) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let mut iris = medulla_rdf::mapping::IriStore::default();
    let triples = medulla_rdf::mapping::parse_turtle(ttl).unwrap();
    medulla_rdf::mapping::load_triples(&core, &mut iris, "g", triples).unwrap();
    core.analysis_snapshot()
}

/// MEDULLA-FEDERATED-QUERY-R709.7: the spatial FILTER planned as a version-checked index
/// scan returns the same rows as the row-by-row scan for every narrowable function over
/// 1,000 fixtures, after inserts and after deletes; EXPLAIN reports `index_used` only when
/// the index reflects the snapshot read, and a stale index falls back to the scan.
/// Verifies MEDULLA-FEDERATED-QUERY-R709.7: Per-graph spatial index over geo literals
// spec: MEDULLA-FEDERATED-QUERY-R709.7
#[test]
fn indexed_and_scanned_results_are_equal() {
    use medulla_rdf::sparql::{execute, spatial_functions, Dataset, Projection};
    use medulla_rdf::spatial_index::SpatialBinding;
    use std::sync::Arc;

    let constants = [
        "POLYGON((20 20, 60 20, 60 60, 20 60, 20 20))",
        "POINT(50 50)",
        "LINESTRING(0 0, 100 100)",
    ];
    let filters = [
        "geof:sfWithin(?w, $C)",
        "geof:sfIntersects(?w, $C)",
        "geof:sfContains(?w, $C)",
        "geof:sfOverlaps(?w, $C)",
        "geof:sfEquals(?w, $C)",
        "geof:sfTouches(?w, $C)",
        "geof:sfCrosses(?w, $C)",
        "geof:sfDisjoint(?w, $C)",
        "geof:distance(?w, $C, <http://www.opengis.net/def/uom/OGC/1.0/degree>) <= 5",
    ];
    let rows = |ds: &Dataset, query: &str| -> Vec<String> {
        let table = execute(ds, query, &Projection::raw(), None)
            .unwrap()
            .into_table();
        let mut out: Vec<String> = table
            .solutions
            .iter()
            .filter_map(|s| s.get("s").map(|b| b.as_str().to_string()))
            .collect();
        out.sort();
        out
    };
    let compare = |view: &medulla_graph_core::graph::GraphView| {
        let current = SpatialBinding::new(Arc::new(ViewAccessor::over(view, 7)), 7);
        let stale = SpatialBinding::new(Arc::new(ViewAccessor::over(view, 6)), 7);
        for constant in constants {
            for filter in filters {
                let literal = format!("\"{constant}\"^^geo:wktLiteral");
                let query = format!(
                    "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                     PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                     SELECT ?s WHERE {{ ?s geo:asWKT ?w . FILTER({}) }}",
                    filter.replace("$C", &literal)
                );
                let plain = Dataset::new(view, Vec::new());
                let indexed = Dataset::new(view, Vec::new()).with_spatial(current.clone());
                let outdated = Dataset::new(view, Vec::new()).with_spatial(stale.clone());
                let scanned = rows(&plain, &query);
                assert_eq!(rows(&indexed, &query), scanned, "{filter} over {constant}");
                assert_eq!(rows(&outdated, &query), scanned, "{filter} stale index");
                let used = |ds: &Dataset| spatial_functions(ds, &query)[0].index_used;
                assert!(used(&indexed), "{filter} index_used");
                assert!(!used(&plain) && !used(&outdated), "{filter} scans");
            }
        }
    };
    compare(&view_of(&fixtures(&Default::default())));
    let deleted = (0..1_000u64).step_by(4).collect();
    compare(&view_of(&fixtures(&deleted)));
}
