//! MEDULLA-FEDERATED-QUERY-R741.1: the hop filter narrows one expansion hop on four axes.
//!
//! The fixed graph: seed `s` has five neighbours. `doc-en` is a `Doc` reached by a `CITES`
//! edge; `doc-de` is a `Doc` literal tagged `de`; `person` is a `Person` reached by a
//! `KNOWS` edge; `other-graph` is a `Doc` that belongs to graph `g2`; `untagged` is a `Doc`
//! literal with no language tag. Each test removes exactly the neighbours that miss one axis.
// spec: MEDULLA-FEDERATED-QUERY-R741.1

use medulla_graph_core::graph::{expand_hops, GraphCore, HopScope};
use medulla_ingestion_types::ingestion_wire::HopFilter;

const GRAPH: &str = "g1";

fn props(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn fixture() -> GraphCore {
    let core = GraphCore::new();
    core.add_node("s".into(), props(serde_json::json!({"type": "Seed"})));
    let neighbours = [
        ("doc-en", serde_json::json!({"type": "Doc", "lang": "en"}), "CITES"),
        ("doc-de", serde_json::json!({"type": "Doc", "lang": "de"}), "CITES"),
        ("person", serde_json::json!({"type": "Person"}), "KNOWS"),
        (
            "other-graph",
            serde_json::json!({"type": "Doc", "graph": "g2"}),
            "CITES",
        ),
        ("untagged", serde_json::json!({"type": "Doc"}), "CITES"),
    ];
    for (id, node, _) in &neighbours {
        core.add_node((*id).into(), props(node.clone()));
    }
    for (id, _, label) in &neighbours {
        core.add_edge(
            "s".into(),
            (*id).into(),
            props(serde_json::json!({"relationship": label})),
        )
        .unwrap();
    }
    core
}

/// The neighbour ids one hop from `s` under `filter`, sorted.
fn walk(core: &GraphCore, filter: &HopFilter) -> Vec<String> {
    let scope = HopScope {
        filter,
        graph_name: GRAPH,
    };
    let mut ids: Vec<String> = expand_hops(core, &[("s".to_string(), 1.0)], 1, 32, 0.5, 64, &scope)
        .into_iter()
        .map(|hit| hit.id)
        .filter(|id| id != "s")
        .collect();
    ids.sort();
    ids
}

#[test]
fn each_axis_filters_one_hop() {
    let core = fixture();

    // No filter restricts nothing.
    assert_eq!(
        walk(&core, &HopFilter::default()),
        ["doc-de", "doc-en", "other-graph", "person", "untagged"]
    );

    // Node labels: only `Person` neighbours.
    let by_node = HopFilter {
        node_labels: vec!["Person".into()],
        ..Default::default()
    };
    assert_eq!(walk(&core, &by_node), ["person"]);

    // Edge labels: only neighbours reached by `KNOWS`.
    let by_edge = HopFilter {
        edge_labels: vec!["KNOWS".into()],
        ..Default::default()
    };
    assert_eq!(walk(&core, &by_edge), ["person"]);

    // Graph: a neighbour with no graph of its own belongs to the walked graph.
    let by_graph = HopFilter {
        graph: Some("g2".into()),
        ..Default::default()
    };
    assert_eq!(walk(&core, &by_graph), ["other-graph"]);
    let own_graph = HopFilter {
        graph: Some(GRAPH.into()),
        ..Default::default()
    };
    assert_eq!(
        walk(&core, &own_graph),
        ["doc-de", "doc-en", "person", "untagged"]
    );

    // Language: a literal with another tag is dropped; one with no tag stays.
    let by_language = HopFilter {
        language: Some("en".into()),
        ..Default::default()
    };
    assert_eq!(
        walk(&core, &by_language),
        ["doc-en", "other-graph", "person", "untagged"]
    );

    // Axes combine by intersection.
    let combined = HopFilter {
        node_labels: vec!["Doc".into()],
        edge_labels: vec!["CITES".into()],
        graph: Some(GRAPH.into()),
        language: Some("de".into()),
    };
    assert_eq!(walk(&core, &combined), ["doc-de", "untagged"]);
}

#[test]
fn a_filtered_hop_fills_its_fan_out_from_matching_neighbours() {
    let core = fixture();
    let scope_filter = HopFilter {
        node_labels: vec!["Doc".into()],
        ..Default::default()
    };
    let scope = HopScope {
        filter: &scope_filter,
        graph_name: GRAPH,
    };
    // A fan-out of two over four `Doc` neighbours keeps two, never the `Person`.
    let hits = expand_hops(&core, &[("s".to_string(), 1.0)], 1, 2, 0.5, 64, &scope);
    let ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
    assert!(!ids.contains(&"person"), "{ids:?}");
}
