//! An agent's knowledge scope read from its definition node (MEDULLA-FEDERATED-QUERY-R704.3):
//! the scope narrows the candidates before truncation and an empty scope is the caller's
//! access alone.

use medulla_core::source_hierarchy::{container_node_id, upsert_containers};
use medulla_graph_core::graph::GraphCore;
use medulla_ingestion_types::source_governance::SourceContainer;
use medulla_retrieval::knowledge_scope::{agent_scope, scope_digest, ResolvedScope};
use medulla_work_types::orchestration::knowledge_scope::KnowledgeScope;
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
        core.add_edge(
            id.to_string(),
            target,
            blob(&json!({"relationship": "inContainer"})),
        )
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
        &[
            container("space", None),
            container("folder", Some("space")),
            container("other", None),
        ],
        0,
    )
    .unwrap();
    record(
        &core,
        "doc-top",
        json!({"source_kind": "confluence"}),
        Some("space"),
    );
    record(
        &core,
        "doc-deep",
        json!({"source_kind": "confluence"}),
        Some("folder"),
    );
    record(
        &core,
        "doc-wrong-kind",
        json!({"source_kind": "slack"}),
        Some("folder"),
    );
    record(
        &core,
        "doc-elsewhere",
        json!({"source_kind": "confluence"}),
        Some("other"),
    );
    record(
        &core,
        "doc-loose",
        json!({"source_kind": "confluence"}),
        None,
    );

    let space = container_node_id("t1", "drive", "space").unwrap();
    core.add_node(
        "agent:support".into(),
        blob(&json!({
            "type": "AgentDefinition",
            "knowledgeScope": {"container_refs": [space], "source_kinds": ["confluence"]},
        })),
    );
    core.add_node(
        "agent:open".into(),
        blob(&json!({"type": "AgentDefinition"})),
    );

    let scope = agent_scope(&core, "agent:support")
        .unwrap()
        .expect("a scope");
    let resolved = ResolvedScope::resolve(&core, &scope);
    let inside: std::collections::BTreeSet<String> = core
        .node_ids()
        .into_iter()
        .filter(|id| resolved.admits(&core, id))
        .collect();
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

    // An agent that declares no scope reads the caller's access alone.
    assert!(agent_scope(&core, "agent:open").unwrap().is_none());
}

// spec: MEDULLA-FEDERATED-QUERY-R704.4
#[test]
fn a_scope_refuses_an_unknown_agent_and_a_foreign_set() {
    let core = GraphCore::new();
    core.add_node("ks:ours".into(), blob(&json!({"type": "KnowledgeSet"})));
    core.add_node(
        "agent:ok".into(),
        blob(&json!({"knowledgeScope": {"knowledge_sets": ["ks:ours"]}})),
    );
    core.add_node(
        "agent:foreign".into(),
        blob(&json!({"knowledgeScope": {"knowledge_sets": ["ks:theirs"]}})),
    );
    assert!(agent_scope(&core, "agent:ok").unwrap().is_some());
    assert_eq!(
        agent_scope(&core, "agent:missing").unwrap_err().code,
        "SCOPE_AGENT_UNKNOWN"
    );
    assert_eq!(
        agent_scope(&core, "agent:foreign").unwrap_err().code,
        "SCOPE_SET_UNKNOWN"
    );
    // A foreign set and an absent one answer byte-identically but for the name asked about.
    core.add_node(
        "agent:absent".into(),
        blob(&json!({"knowledgeScope": {"knowledge_sets": ["ks:nowhere"]}})),
    );
    let foreign = agent_scope(&core, "agent:foreign").unwrap_err();
    let absent = agent_scope(&core, "agent:absent").unwrap_err();
    assert_eq!(foreign.code, absent.code);
    assert_eq!(
        foreign.detail.replace("ks:theirs", "SET"),
        absent.detail.replace("ks:nowhere", "SET")
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R704.5
#[test]
fn the_scope_digest_is_canonical_and_the_empty_scope_has_the_empty_record_digest() {
    let one = KnowledgeScope {
        agent_ref: "agent:a".into(),
        source_kinds: vec!["slack".into(), "confluence".into(), "slack".into()],
        ..KnowledgeScope::default()
    };
    let other = KnowledgeScope {
        agent_ref: "agent:b".into(),
        source_kinds: vec!["confluence".into(), "slack".into()],
        ..KnowledgeScope::default()
    };
    // Lists are sorted and de-duplicated, and the agent is not part of the digest.
    assert_eq!(scope_digest(Some(&one)), scope_digest(Some(&other)));
    assert_ne!(scope_digest(Some(&one)), scope_digest(None));
    let empty = medulla_types::retrieval::sha256_hex(
        br#"{"container_refs":[],"knowledge_sets":[],"source_kinds":[]}"#,
    );
    assert_eq!(scope_digest(None), empty);
    assert_eq!(scope_digest(Some(&KnowledgeScope::default())), empty);
}
