//! Round-trip coverage for the architecture-view wire model.

use medulla_types::architecture_view::{
    ArchitectureView, ViewAbstained, ViewContainer, ViewEdge, ViewScope,
};

// spec: MEDULLA-CQ-ORCH-R010.1
#[test]
fn view_types_round_trip() {
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

    let scope = ViewScope {
        repository_id: "repo-1".to_owned(),
        revision: Some("ab".repeat(32)),
    };
    assert!(scope.revision_is_valid());
    let scope_back: ViewScope =
        serde_json::from_value(serde_json::to_value(&scope).unwrap()).unwrap();
    assert_eq!(scope_back, scope);
    let latest: ViewScope =
        serde_json::from_value(serde_json::json!({"repository_id": "repo-1"})).unwrap();
    assert_eq!(latest.revision, None);
    assert!(latest.revision_is_valid());
    for bad in ["short", &"AB".repeat(32)] {
        let bad_scope = ViewScope { repository_id: "r".into(), revision: Some((*bad).into()) };
        assert!(!bad_scope.revision_is_valid());
    }
    assert!(serde_json::from_value::<ViewScope>(
        serde_json::json!({"repository_id": "r", "branch": "main"})
    )
    .is_err());
    for refusal in [
        ViewAbstained::NoSupportingSource,
        ViewAbstained::UnknownRepository,
        ViewAbstained::StaleRevision,
        ViewAbstained::TooLarge,
    ] {
        let back: ViewAbstained =
            serde_json::from_value(serde_json::to_value(refusal).unwrap()).unwrap();
        assert_eq!(back, refusal);
    }
    assert_eq!(ViewAbstained::TooLarge.code(), "VIEW_TOO_LARGE");
}
