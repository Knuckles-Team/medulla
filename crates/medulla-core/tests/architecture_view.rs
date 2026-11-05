//! Round-trip coverage for the architecture-view wire model.

use medulla_core::architecture_view::{
    derive_architecture_view, derive_architecture_view_with, ViewRules,
};
use medulla_graph_core::graph::GraphCore;
use medulla_types::architecture_view::{
    check_revision, view_digest, ArchitectureView, ArchitectureViewResponse, ViewAbstained,
    ViewContainer, ViewEdge, ViewScope, REFUSALS,
};
use serde_json::json;

fn node(core: &GraphCore, id: &str, props: serde_json::Value) {
    core.add_node(
        id.to_owned(),
        rmp_serde::to_vec_named(&props).expect("encode"),
    );
}

fn calls(core: &GraphCore, from: &str, to: &str) {
    core.add_edge(
        from.to_owned(),
        to.to_owned(),
        rmp_serde::to_vec_named(&json!({"relationship": "calls"})).expect("encode"),
    )
    .expect("edge");
}

fn scope() -> ViewScope {
    ViewScope {
        repository_id: "repo".to_owned(),
        revision: None,
    }
}

/// Three packages, cross-package calls, and two ArchiMate components: one that
/// matches a package by case-folded name and one diagram-only.
fn fixture() -> GraphCore {
    let core = GraphCore::new();
    for (id, file) in [
        ("m-orders", "services/orders/Cargo.toml"),
        ("m-billing", "services/billing/pyproject.toml"),
        ("m-web", "web/package.json"),
    ] {
        node(
            &core,
            id,
            json!({"type": "FILE", "file": file, "repository_id": "repo"}),
        );
    }
    for (id, file) in [
        ("s-orders", "services/orders/src/main.rs"),
        ("s-orders2", "services/orders/src/lib.rs"),
        ("s-billing", "services/billing/app.py"),
        ("s-web", "web/src/index.js"),
    ] {
        node(
            &core,
            id,
            json!({"type": "SYMBOL", "file": file, "repository_id": "repo"}),
        );
    }
    calls(&core, "s-orders", "s-billing");
    calls(&core, "s-orders2", "s-billing");
    calls(&core, "s-web", "s-orders");
    calls(&core, "s-orders", "s-orders2");
    node(
        &core,
        "d-orders",
        json!({"type": "ApplicationComponent", "name": "ORDERS", "repository_id": "repo"}),
    );
    node(
        &core,
        "d-ledger",
        json!({"type": "ApplicationComponent", "name": "ledger", "repository_id": "repo"}),
    );
    core
}

// spec: MEDULLA-CQ-ORCH-R010.1
#[test]
fn view_types_round_trip() {
    let scope = ViewScope {
        repository_id: "repo".to_owned(),
        revision: Some("a".repeat(64)),
    };
    let encoded = serde_json::to_string(&scope).expect("scope serializes");
    assert_eq!(
        serde_json::from_str::<ViewScope>(&encoded).expect("scope"),
        scope
    );
    for refusal in [
        ViewAbstained::NoSupportingSource,
        ViewAbstained::UnknownRepository,
        ViewAbstained::StaleRevision {
            stated: "a".into(),
            newest: "b".into(),
        },
        ViewAbstained::TooLarge {
            containers: 3,
            limit: 2,
        },
    ] {
        let encoded = serde_json::to_string(&refusal).expect("refusal serializes");
        assert_eq!(
            serde_json::from_str::<ViewAbstained>(&encoded).expect("refusal"),
            refusal
        );
        assert!(REFUSALS.contains(&refusal.code()));
    }

    let expected = ArchitectureView {
        containers: vec![ViewContainer {
            name: "orders-api".to_owned(),
            technology: "Rust service".to_owned(),
            support: vec![
                "services/orders/src/main.rs".to_owned(),
                "deploy/orders.yaml".to_owned(),
            ],
        }],
        edges: vec![ViewEdge {
            from: "orders-api".to_owned(),
            to: "payments-api".to_owned(),
            kind: "calls".to_owned(),
            call_count: 3,
        }],
    };

    let encoded = serde_json::to_vec(&expected).expect("architecture view serializes");
    let decoded: ArchitectureView =
        serde_json::from_slice(&encoded).expect("architecture view deserializes");

    assert_eq!(decoded, expected);
}

// spec: MEDULLA-CQ-ORCH-R010.2
#[test]
fn derivation_merges_code_and_diagram_elements() {
    let core = fixture();
    let view = derive_architecture_view(&core, &scope()).expect("view");
    let names: Vec<_> = view.containers.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["billing", "ledger", "orders", "web"]);
    let tech: Vec<_> = view
        .containers
        .iter()
        .map(|c| c.technology.as_str())
        .collect();
    assert_eq!(tech, ["Python", "", "Rust", "Node"]);
    assert!(
        view.containers[1].support.is_empty(),
        "diagram-only container"
    );
    assert_eq!(
        view.edges,
        vec![
            ViewEdge {
                from: "orders".into(),
                to: "billing".into(),
                kind: "calls".into(),
                call_count: 2
            },
            ViewEdge {
                from: "web".into(),
                to: "orders".into(),
                kind: "calls".into(),
                call_count: 1
            },
        ]
    );
    assert_eq!(
        derive_architecture_view(&core, &scope()).expect("again"),
        view
    );
    let tight = ViewRules {
        max_containers: 3,
        ..ViewRules::default()
    };
    assert_eq!(
        derive_architecture_view_with(&core, &scope(), &tight),
        Err(ViewAbstained::TooLarge {
            containers: 4,
            limit: 3
        })
    );
}

// spec: MEDULLA-CQ-ORCH-R010.4
#[test]
fn empty_scope_abstains() {
    let core = GraphCore::new();
    node(&core, "x", json!({"type": "Note", "repository_id": "repo"}));
    assert_eq!(
        derive_architecture_view(&core, &scope()),
        Err(ViewAbstained::NoSupportingSource)
    );
    let other = ViewScope {
        repository_id: "elsewhere".into(),
        revision: None,
    };
    assert_eq!(
        derive_architecture_view(&core, &other),
        Err(ViewAbstained::UnknownRepository)
    );
    let stale = ViewScope {
        revision: Some("a".repeat(64)),
        ..scope()
    };
    assert_eq!(
        check_revision(&stale, Some(&"b".repeat(64))).map_err(|e| e.code()),
        Err("VIEW_STALE_REVISION")
    );
    assert_eq!(check_revision(&stale, Some(&"a".repeat(64))), Ok(()));
}

// spec: MEDULLA-CQ-ORCH-R010.5
#[test]
fn every_container_cites_support() {
    let core = fixture();
    let view = derive_architecture_view(&core, &scope()).expect("view");
    let orders = view
        .containers
        .iter()
        .find(|c| c.name == "orders")
        .expect("orders");
    assert_eq!(
        orders.support,
        [
            "services/orders/Cargo.toml",
            "services/orders/src/lib.rs",
            "services/orders/src/main.rs"
        ]
    );
    for container in view.containers.iter().filter(|c| !c.technology.is_empty()) {
        assert!(
            !container.support.is_empty(),
            "{} cites no file",
            container.name
        );
    }
    let response = ArchitectureViewResponse::new(view.clone(), Some("c".repeat(64)));
    assert_eq!(response.digest.len(), 64);
    assert!(response
        .digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(response.digest, view_digest(&view));
}
