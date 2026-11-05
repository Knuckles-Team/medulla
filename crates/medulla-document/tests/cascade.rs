//! Document delete cascade (MEDULLA-UNIFIED-DATA-PLANE-R651.1/.2).

use medulla_document::cascade::{
    plan_cascade, CascadePlan, CascadeReader, CascadeRefusal, EdgeKey, NodeKey, OrphanReport,
    CASCADE_TOO_LARGE,
};
use medulla_types::document_ops::{DocumentDeleteReceipt, DocumentDeleteRequest};

fn node(label: &str, id: &str) -> NodeKey {
    NodeKey {
        label: label.into(),
        id: id.into(),
    }
}

#[derive(Default)]
struct Graph {
    edges: Vec<EdgeKey>,
}

impl Graph {
    fn link(&mut self, from: NodeKey, label: &str, to: NodeKey) {
        self.edges.push(EdgeKey {
            from,
            label: label.into(),
            to,
        });
    }
}

impl CascadeReader for Graph {
    fn in_edges(&self, node: &NodeKey) -> Vec<EdgeKey> {
        self.edges
            .iter()
            .filter(|e| &e.to == node)
            .cloned()
            .collect()
    }
    fn out_edges(&self, node: &NodeKey) -> Vec<EdgeKey> {
        self.edges
            .iter()
            .filter(|e| &e.from == node)
            .cloned()
            .collect()
    }
}

fn document(chunks: usize) -> Graph {
    let mut graph = Graph::default();
    let doc = node("Document", "doc-1");
    for i in 0..chunks {
        let chunk = node("Chunk", &format!("c{i:04}"));
        graph.link(chunk.clone(), "chunkOf", doc.clone());
        graph.link(
            node("Fact", &format!("f{i:04}")),
            "extractedFrom",
            chunk.clone(),
        );
        graph.link(chunk, "mentions", node("Entity", "shared"));
    }
    graph.link(node("Enrichment", "e1"), "enrichmentOf", doc.clone());
    graph.link(doc, "fromSource", node("Source", "s1"));
    graph
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.1
#[test]
fn cascade_types_round_trip() {
    let plan = plan_cascade(&document(3), "t1", "doc-1", 100).unwrap();
    assert_eq!(plan.plan_digest.len(), 64);
    assert!(plan
        .plan_digest
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    let json = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        serde_json::from_value::<CascadePlan>(json.clone()).unwrap(),
        plan
    );

    let mut unsorted = json.clone();
    unsorted["nodes"].as_array_mut().unwrap().reverse();
    assert!(serde_json::from_value::<CascadePlan>(unsorted).is_err());
    let mut tampered = json.clone();
    tampered["plan_digest"] = serde_json::json!("0".repeat(64));
    assert!(serde_json::from_value::<CascadePlan>(tampered).is_err());
    let mut extra = json;
    extra["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CascadePlan>(extra).is_err());

    let report = OrphanReport {
        graph_rows_left: 0,
        edges_left: 0,
        index_pending: 4,
    };
    let back: OrphanReport = serde_json::from_value(serde_json::to_value(report).unwrap()).unwrap();
    assert_eq!(back, report);
    let request = DocumentDeleteRequest {
        tenant: "t1".into(),
        document_id: "doc-1".into(),
    };
    let back: DocumentDeleteRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(back, request);
    let receipt = DocumentDeleteReceipt {
        tenant: "t1".into(),
        plan_digest: plan.plan_digest.clone(),
        nodes_deleted: plan.nodes.len() as u64,
        edges_deleted: plan.edges.len() as u64,
        index_pending: 0,
    };
    let back: DocumentDeleteReceipt =
        serde_json::from_value(serde_json::to_value(&receipt).unwrap()).unwrap();
    assert_eq!(back, receipt);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.2
#[test]
fn plan_is_deterministic_and_complete() {
    let graph = document(500);
    let first = plan_cascade(&graph, "t1", "doc-1", 2_000).unwrap();
    let second = plan_cascade(&graph, "t1", "doc-1", 2_000).unwrap();
    assert_eq!(first, second);
    // Document + 500 chunks + 500 facts + 1 enrichment; shared entity and source survive.
    assert_eq!(first.nodes.len(), 1_002);
    assert!(!first.nodes.contains(&node("Entity", "shared")));
    assert!(!first.nodes.contains(&node("Source", "s1")));
    // Every edge touching a planned node is deleted.
    assert_eq!(first.edges.len(), graph.edges.len());
    let threaded: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    plan_cascade(&graph, "t1", "doc-1", 2_000)
                        .unwrap()
                        .plan_digest
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(threaded.iter().all(|digest| digest == &first.plan_digest));
    let refusal = plan_cascade(&graph, "t1", "doc-1", 100).unwrap_err();
    assert_eq!(refusal, CascadeRefusal::TooLarge { max_nodes: 100 });
    assert_eq!(refusal.code(), CASCADE_TOO_LARGE);
}
