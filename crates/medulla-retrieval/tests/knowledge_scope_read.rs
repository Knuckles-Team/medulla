//! An agent's knowledge scope read from its definition node (MEDULLA-FEDERATED-QUERY-R704.3):
//! the scope narrows the candidates before truncation and an empty scope is the caller's
//! access alone.

use medulla_core::source_hierarchy::{container_node_id, upsert_containers};
use medulla_graph_core::graph::GraphCore;
use medulla_ingestion_types::source_governance::SourceContainer;
use medulla_retrieval::knowledge_scope::{agent_scope, ResolvedScope};
use serde_json::{json, Value};

fn blob(value: &Value) -> Vec<u8> {
    rmp_serde::to_vec_named(value).unwrap()
}

fn container(id: &str, parent: Option<&str>) -> SourceContainer {
    SourceContainer {
        source: "drive".into(),
        container_id: id.into(),
        parent_id: parent.map(str::to_string),
        title: id.into(),
    }
}

fn record(core: &GraphCore, id: &str, props: Value, container: Option<&str>) {
    core.add_node(id.to_string(), blob(&props));
    if let Some(container) = container {
        let target = container_node_id("t1", "drive", container).unwrap();
        core.add_edge(id.to_string(), target, blob(&json!({"relationship": "inContainer"})))
            .unwrap();
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R704.3
#[test]
fn an_agent_scope_narrows_by_container_subtree_and_source_kind() {
    let core = GraphCore::new();
    upsert_containers(
        &core,
        "t1",
        &[container("space", None), container("folder", Some("space")), container("other", None)],
    )
    .unwrap();
    record(&core, "doc-top", json!({"source_kind": "confluence"}), Some("space"));
    record(&core, "doc-deep", json!({"source_kind": "confluence"}), Some("folder"));
    record(&core, "doc-wrong-kind", json!({"source_kind": "slack"}), Some("folder"));
    record(&core, "doc-elsewhere", json!({"source_kind": "confluence"}), Some("other"));
    record(&core, "doc-loose", json!({"source_kind": "confluence"}), None);

    let space = container_node_id("t1", "drive", "space").unwrap();
    core.add_node(
        "agent:support".into(),
        blob(&json!({
            "type": "AgentDefinition",
            "knowledgeScope": {"container_refs": [space], "source_kinds": ["confluence"]},
        })),
    );
    core.add_node("agent:open".into(), blob(&json!({"type": "AgentDefinition"})));

    let scope = agent_scope(&core, "agent:support").unwrap().expect("a scope");
    let resolved = ResolvedScope::resolve(&core, &scope);
    let inside = resolved.node_ids(&core);
    assert!(inside.contains("doc-top") && inside.contains("doc-deep"));
    assert!(!inside.contains("doc-wrong-kind"));
    assert!(!inside.contains("doc-elsewhere") && !inside.contains("doc-loose"));

    // The scope cuts before the top k: the best out-of-scope hit never takes a slot.
    let scored = vec![
        ("doc-elsewhere".to_string(), 0.99),
        ("doc-deep".to_string(), 0.5),
        ("doc-top".to_string(), 0.4),
    ];
    let kept = resolved.top_k(&core, scored, 2);
    assert_eq!(
        kept.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["doc-deep", "doc-top"]
    );

    // An agent that declares no scope, and an agent with no node, read the caller's access alone.
    assert!(agent_scope(&core, "agent:open").unwrap().is_none());
    assert!(agent_scope(&core, "agent:missing").unwrap().is_none());
}
