//! Served `SpecStatus` reads over a real graph (MEDULLA-SDLC-R005.3, R005.4).

use medulla_core::lifecycle::status::answer_spec_status;
use medulla_graph_core::graph::GraphCore;
use medulla_source_types::lifecycle::{LifecycleRefusal, SpecState};
use serde_json::{json, Value};

use crate::lifecycle_status::spec_status_view;

fn props(value: Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).expect("properties encode")
}

fn requirement(core: &GraphCore, repo: &str, row: &str, tenant: &str) -> String {
    let id = format!("requirement:{repo}:{row}");
    core.add_node(
        id.clone(),
        props(json!({"type": "Requirement", "repo": repo, "rowId": row, "tenant_id": tenant})),
    );
    id
}

fn changeset(core: &GraphCore, repo: &str, id: &str, at: u64, tenant: &str, cites: &[&str]) {
    let node = format!("changeset:{repo}:{id}");
    core.add_node(
        node.clone(),
        props(json!({
            "type": "Changeset", "repo": repo, "changeset_id": id,
            "created_at": at.to_string(), "tenant_id": tenant,
        })),
    );
    for row in cites {
        core.add_typed_edge(
            node.clone(),
            format!("requirement:{repo}:{row}"),
            "citesRequirement".into(),
            props(json!({})),
        )
        .expect("edge adds");
    }
}

fn bind(core: &GraphCore, repo: &str, row: &str, path: &str, tenant: &str) {
    let test = format!("testfile:{repo}:{path}");
    core.add_node(
        test.clone(),
        props(json!({"type": "TestFile", "repo": repo, "path": path, "tenant_id": tenant})),
    );
    core.add_typed_edge(
        format!("requirement:{repo}:{row}"),
        test,
        "boundTest".into(),
        props(json!({})),
    )
    .expect("edge adds");
}

fn everything(_blob: &[u8], _schema: bool) -> bool {
    true
}

fn fixture(core: &GraphCore, tenant: &str) {
    requirement(core, "demo", "DEMO-A-R001", tenant);
    requirement(core, "demo", "DEMO-A-R002", tenant);
    changeset(core, "demo", "c1", 1, tenant, &["DEMO-A-R001"]);
    changeset(core, "demo", "c2", 2, tenant, &["DEMO-A-R002"]);
    bind(core, "demo", "DEMO-A-R001", "tests/test_a.py", tenant);
}

// spec: MEDULLA-SDLC-R005.3
#[test]
fn spec_status_is_tenant_scoped() {
    let core = GraphCore::new();
    fixture(&core, "acme");
    let mine = spec_status_view(&core, "acme", "demo", &everything);
    let answer = answer_spec_status("demo", "c2", None, &mine).expect("answers");
    let state = |id: &str| answer.rows.iter().find(|r| r.id == id).map(|r| r.state);
    assert_eq!(state("DEMO-A-R001"), Some(SpecState::Verified));
    assert_eq!(state("DEMO-A-R002"), Some(SpecState::Landed));
    let theirs = spec_status_view(&core, "globex", "demo", &everything);
    assert!(theirs.rows.is_empty() && theirs.changesets.is_empty());
    assert!(matches!(
        answer_spec_status("demo", "c2", None, &theirs),
        Err(LifecycleRefusal::SpecTreeAbsent(_))
    ));
    let hidden = spec_status_view(&core, "acme", "demo", &|_: &[u8], _: bool| false);
    assert!(hidden.rows.is_empty());
}

// spec: MEDULLA-SDLC-R005.3
#[test]
fn spec_status_reads_the_chain_up_to_rev_and_filters_by_spec() {
    let core = GraphCore::new();
    fixture(&core, "acme");
    requirement(&core, "demo", "OTHER-R001", "acme");
    let view = spec_status_view(&core, "acme", "demo", &everything);
    let early = answer_spec_status("demo", "c1", Some("DEMO-A"), &view).expect("answers");
    assert_eq!(early.rows.len(), 2);
    let second = early
        .rows
        .iter()
        .find(|r| r.id == "DEMO-A-R002")
        .expect("row");
    assert_eq!(second.state, SpecState::Specified);
    assert!(early.rows.iter().all(|r| r.id.starts_with("DEMO-A-R")));
}

// spec: MEDULLA-SDLC-R005.4
#[test]
fn spec_status_refusals() {
    let core = GraphCore::new();
    fixture(&core, "acme");
    let view = spec_status_view(&core, "acme", "demo", &everything);
    let unknown = answer_spec_status("demo", "nope", None, &view).expect_err("refuses");
    assert_eq!(unknown.code(), "SPEC_STATUS_REV_UNKNOWN");
    let absent = spec_status_view(&core, "acme", "elsewhere", &everything);
    let refused = answer_spec_status("elsewhere", "c1", None, &absent).expect_err("refuses");
    assert_eq!(refused.code(), "SPEC_TREE_ABSENT");
    assert!(refused.to_string().starts_with("SPEC_TREE_ABSENT: "));
}

fn lifecycle_node(core: &GraphCore, id: &str, node_type: &str, tenant: &str) {
    core.add_node(
        id.into(),
        props(json!({"type": node_type, "tenant_id": tenant})),
    );
}

fn lifecycle_edge(core: &GraphCore, from: &str, relationship: &str, to: &str) {
    core.add_typed_edge(
        from.into(),
        to.into(),
        relationship.into(),
        props(json!({"rule": "r-test", "rule_version": "3", "input_digests": "d1,d2"})),
    )
    .expect("edge adds");
}

// spec: MEDULLA-SDLC-R006.3
#[test]
fn trace_is_tenant_scoped() {
    use crate::lifecycle_status::lifecycle_store_around;
    use medulla_core::lifecycle::trace::lifecycle_trace;
    use medulla_source_types::lifecycle::{LifecycleTraceRequest, TraceLimits};
    let core = GraphCore::new();
    lifecycle_node(&core, "requirement:d:R1", "Requirement", "acme");
    lifecycle_node(&core, "symbol:s", "SYMBOL", "acme");
    lifecycle_node(&core, "commit:c1", "Changeset", "acme");
    lifecycle_edge(&core, "symbol:s", "implements", "requirement:d:R1");
    lifecycle_edge(&core, "commit:c1", "changes", "symbol:s");
    let limits = TraceLimits {
        max_depth: 8,
        node_budget: 100,
    };
    let request = LifecycleTraceRequest::new("requirement:d:R1".into(), 2, vec![]).unwrap();
    let walk = |tenant: &str| {
        let store = lifecycle_store_around(&core, tenant, &request.anchor, 2, 100, &everything);
        lifecycle_trace(&store, &request, limits)
    };
    let mine = walk("acme");
    assert_eq!(mine.records.len(), 3);
    assert!(mine
        .edges
        .iter()
        .all(|e| e.rule == "r-test" && e.rule_version == 3));
    let theirs = walk("globex");
    assert!(theirs.records.is_empty() && theirs.edges.is_empty());
}

// spec: MEDULLA-SDLC-R006.4
#[test]
fn trace_refusals() {
    use medulla_source_types::lifecycle::{
        handle_lifecycle_trace, LifecycleKind, LifecycleTraceRequest, TraceLimits,
    };
    let limits = TraceLimits {
        max_depth: 4,
        node_budget: 100,
    };
    let deep = LifecycleTraceRequest::new("a".into(), 5, vec![]).unwrap();
    assert_eq!(
        handle_lifecycle_trace(deep, limits).unwrap_err().code(),
        "LIFECYCLE_DEPTH_INVALID"
    );
    assert_eq!(
        LifecycleTraceRequest::new("a".into(), 0, vec![])
            .unwrap_err()
            .code(),
        "LIFECYCLE_DEPTH_INVALID"
    );
    assert_eq!(
        LifecycleKind::parse("widget").unwrap_err().code(),
        "LIFECYCLE_KIND_UNKNOWN"
    );
}
