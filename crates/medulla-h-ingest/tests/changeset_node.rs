//! The changeset node of an accept (MEDULLA-SDLC-R001.3).

use medulla_core::lifecycle::accept::{accept_changeset_edges, graph_endpoint};
use medulla_core::lifecycle::derive::CodeGraphView;
use medulla_core::lifecycle::store::LifecycleStore;
use medulla_source_types::lifecycle::{
    plan_changeset_node, LifecycleEdgeKind, LifecycleKind, StoredChangesetNode,
};
use medulla_source_types::source_changeset::{commit_node_id, Changeset, Trailers, COMMIT_TYPE};

fn changeset() -> Changeset {
    Changeset {
        id: "c1".into(),
        repo: "demo".into(),
        parents: vec!["c0".into()],
        author_principal: "alice".into(),
        message: "m".into(),
        changed_paths: Vec::new(),
        created_at: 7,
        trailers: Trailers {
            spec_ids: vec!["DEMO-R001".into()],
            fixes: Vec::new(),
            withdraw: Vec::new(),
            withdraw_reason: None,
        },
        agent_identity: None,
        run_trace_id: None,
        git: None,
    }
}

// spec: MEDULLA-SDLC-R001.3
#[test]
fn changeset_node_is_written_once_with_edges_and_reindex_is_idempotent() {
    let changeset = changeset();
    let requirement = "requirement:demo:DEMO-R001";
    let known = |id: &str| (id == requirement).then_some(LifecycleKind::Requirement);

    // One accept writes `commit:<id>` with every property, and its derived edge between the
    // graph node ids.
    let node = plan_changeset_node(&changeset, 2, None, true).expect("fresh node plans");
    assert_eq!(node.node_id, commit_node_id("c1"));
    let properties = node.properties();
    assert_eq!(properties["type"], COMMIT_TYPE);
    assert_eq!(properties["changeset_id"], "c1");
    assert_eq!(properties["seq"], "2");
    assert_eq!(properties["repo"], "demo");
    assert_eq!(properties["author"], "alice");
    assert_eq!(properties["timestamp_ms"], "7");
    assert_eq!(properties["parents"], "c0");
    let writes = accept_changeset_edges(
        &mut LifecycleStore::default(),
        &changeset,
        &CodeGraphView::default(),
        node.clone(),
        &known,
    )
    .expect("accept");
    let cites: Vec<_> = writes
        .edges
        .iter()
        .map(|edge| (edge.kind, edge.from.as_str(), edge.to.as_str()))
        .collect();
    assert_eq!(
        cites,
        vec![(LifecycleEdgeKind::CitesRequirement, "commit:c1", requirement)]
    );
    assert_eq!(
        graph_endpoint(&changeset, LifecycleEdgeKind::CitesRequirement, false, "DEMO-R001"),
        requirement
    );

    // A second accept and a re-index with equal values plan the same node.
    let stored = StoredChangesetNode {
        changeset_id: Some("c1".into()),
        seq: Some("2".into()),
    };
    assert_eq!(
        plan_changeset_node(&changeset, 2, Some(&stored), true).expect("equal values"),
        node
    );
    // On the index route the node gets only `type`, `changeset_id` and `seq`: no second writer.
    let index = plan_changeset_node(&changeset, 2, Some(&stored), false).expect("index route");
    let keys: Vec<_> = index.properties().into_keys().collect();
    assert_eq!(keys, vec!["changeset_id", "seq", "type"]);

    // A differing `seq` or `changeset_id` refuses and writes nothing.
    let moved = StoredChangesetNode {
        changeset_id: Some("c1".into()),
        seq: Some("3".into()),
    };
    let refusal = plan_changeset_node(&changeset, 2, Some(&moved), true).expect_err("conflict");
    assert_eq!(refusal.code(), "CHANGESET_NODE_CONFLICT");

    // A cited requirement that is not ingested yet drops its edge and is reported.
    let waiting = accept_changeset_edges(
        &mut LifecycleStore::default(),
        &changeset,
        &CodeGraphView::default(),
        node.clone(),
        &|_: &str| None,
    )
    .expect("a trailer edge waits for its record");
    assert!(waiting.edges.is_empty());
    assert_eq!(
        waiting.unresolved,
        vec![(LifecycleEdgeKind::CitesRequirement, requirement.to_string())]
    );
    // A run that produced the changeset must exist: an absent endpoint refuses the write.
    let mut produced = changeset.clone();
    produced.run_trace_id = Some("run-1".into());
    let refusal = accept_changeset_edges(
        &mut LifecycleStore::default(),
        &produced,
        &CodeGraphView::default(),
        node,
        &known,
    )
    .expect_err("unknown run");
    assert_eq!(refusal.code(), "LIFECYCLE_ENDPOINT_UNKNOWN");
}
