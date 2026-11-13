//! Source container hierarchy (MEDULLA-INGEST-WORKER-R904.2).

use medulla_core::source_hierarchy::{
    container_node_id, subtree, upsert_containers, ContainerUpsert, CONTAINER_NODE_PREFIX,
};
use medulla_graph_core::graph::GraphCore;
use medulla_types::source_governance::SourceContainer;

fn container(id: &str, parent: Option<&str>) -> SourceContainer {
    SourceContainer {
        source: "drive".into(),
        container_id: id.into(),
        parent_id: parent.map(str::to_string),
        title: format!("title {id}"),
    }
}

fn node(id: &str) -> String {
    container_node_id("t1", "drive", id).unwrap()
}

fn tree() -> Vec<SourceContainer> {
    vec![
        container("root", None),
        container("b", Some("root")),
        container("a", Some("root")),
        container("a2", Some("a")),
        container("a1", Some("a")),
        container("b1", Some("b")),
    ]
}

// spec: MEDULLA-INGEST-WORKER-R904.2
#[test]
fn subtree_returns_descendants() {
    let core = GraphCore::new();
    // Children listed before their parents are fine: the batch is one unit.
    let mut batch = tree();
    batch.reverse();
    let written = upsert_containers(&core, "t1", &batch).unwrap();
    assert_eq!(written, ContainerUpsert { nodes: 6, edges: 5 });
    assert!(node("root").starts_with(CONTAINER_NODE_PREFIX));

    // Three levels, ordered by depth then container id.
    let all = subtree(&core, &node("root"), 32).unwrap();
    let order: Vec<(&str, usize)> = all
        .iter()
        .map(|e| (e.container_id.as_str(), e.depth))
        .collect();
    assert_eq!(
        order,
        vec![
            ("root", 0),
            ("a", 1),
            ("b", 1),
            ("a1", 2),
            ("a2", 2),
            ("b1", 2)
        ]
    );

    // A subtree under one folder holds only its descendants; a leaf holds itself.
    let under_a: Vec<String> = subtree(&core, &node("a"), 32)
        .unwrap()
        .into_iter()
        .map(|e| e.container_id)
        .collect();
    assert_eq!(under_a, vec!["a", "a1", "a2"]);
    assert_eq!(subtree(&core, &node("b1"), 32).unwrap().len(), 1);

    // max_depth bounds the walk.
    assert_eq!(subtree(&core, &node("root"), 0).unwrap().len(), 1);
    assert_eq!(subtree(&core, &node("root"), 1).unwrap().len(), 3);

    // An unknown root is refused.
    assert_eq!(
        subtree(&core, "source-container:nope", 3).unwrap_err().code,
        "CONTAINER_NOT_FOUND"
    );
}

// spec: MEDULLA-INGEST-WORKER-R904.2
#[test]
fn upsert_is_idempotent_reparents_and_refuses_bad_trees() {
    let core = GraphCore::new();
    upsert_containers(&core, "t1", &tree()).unwrap();
    // Re-sending the tree changes nothing.
    upsert_containers(&core, "t1", &tree()).unwrap();
    assert_eq!(subtree(&core, &node("root"), 32).unwrap().len(), 6);

    // A parent outside the batch that is already held is accepted; a move drops the old edge.
    upsert_containers(&core, "t1", &[container("b1", Some("a"))]).unwrap();
    let under_b: Vec<String> = subtree(&core, &node("b"), 32)
        .unwrap()
        .into_iter()
        .map(|e| e.container_id)
        .collect();
    assert_eq!(under_b, vec!["b"]);
    let under_a: Vec<String> = subtree(&core, &node("a"), 32)
        .unwrap()
        .into_iter()
        .map(|e| e.container_id)
        .collect();
    assert_eq!(under_a, vec!["a", "a1", "a2", "b1"]);

    // Refusals write nothing.
    let before = core.get_nodes().len();
    let cycle = upsert_containers(
        &core,
        "t1",
        &[container("x", Some("y")), container("y", Some("x"))],
    )
    .unwrap_err();
    assert!(cycle.code.contains("CONTAINER"), "{}", cycle.code);
    let orphan = upsert_containers(&core, "t1", &[container("z", Some("ghost"))]).unwrap_err();
    assert_eq!(orphan.code, "CONTAINER_PARENT_UNKNOWN");
    let selfie = upsert_containers(&core, "t1", &[container("s", Some("s"))]).unwrap_err();
    assert_eq!(selfie.code, "CONTAINER_INVALID");
    assert_eq!(core.get_nodes().len(), before);

    // Another tenant's containers are a different node set.
    assert_ne!(
        node("root"),
        container_node_id("t2", "drive", "root").unwrap()
    );
    assert!(subtree(&core, &container_node_id("t2", "drive", "root").unwrap(), 3).is_err());
}
