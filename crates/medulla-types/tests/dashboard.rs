//! The dashboard asset record (MEDULLA-TYPED-PACKS-R109.1).

use medulla_types::dashboard::{
    DashboardAsset, DashboardRole, DASHBOARD_BODY_TOO_LARGE, DASHBOARD_REVISION_STALE,
};
use serde_json::json;

const CLASSES: &str = include_str!("../../medulla-core/ontology/metadata_catalog-v1.ttl");
const SHAPES: &str = include_str!("../../medulla-core/ontology/metadata_catalog-v1.shapes.ttl");

fn asset() -> DashboardAsset {
    serde_json::from_value(json!({
        "id": "dash-1",
        "body": {
            "panels": [{
                "id": "p1", "source": "orders", "query": "SELECT 1",
                "terms": ["Order"], "concepts": ["core:Order"]
            }],
            "layout": {"columns": 2},
            "parameters": {"window": "7d"}
        },
        "revision": 3,
        "owner": "principal:alice",
        "grants": [{"principal_or_group": "group:sales", "role": "view"}]
    }))
    .expect("a dashboard asset parses")
}

// spec: MEDULLA-TYPED-PACKS-R109.1
#[test]
fn dashboard_asset_round_trips_and_shape_validates() {
    let dashboard = asset();
    dashboard.validate().expect("valid");
    assert_eq!(dashboard.grants[0].role, DashboardRole::View);
    let wire = serde_json::to_value(&dashboard).unwrap();
    let back: DashboardAsset = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, dashboard);

    // Unknown fields and unknown roles are refused.
    let mut extra = wire.clone();
    extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<DashboardAsset>(extra).is_err());
    let mut role = wire;
    role["grants"][0]["role"] = json!("admin");
    assert!(serde_json::from_value::<DashboardAsset>(role).is_err());

    // A grant needs a principal and is listed once; an oversized body is refused.
    let mut twice = asset();
    twice.grants.push(twice.grants[0].clone());
    assert!(twice.validate().is_err());
    let mut big = asset();
    big.body.panels[0].query = "x".repeat(2_000_000);
    assert!(big.validate().unwrap_err().starts_with(DASHBOARD_BODY_TOO_LARGE));

    // The revision only rises.
    let mut next = asset();
    assert!(next
        .check_successor_of(&dashboard)
        .unwrap_err()
        .starts_with(DASHBOARD_REVISION_STALE));
    next.revision = 4;
    next.check_successor_of(&dashboard).expect("rises");

    // The class sits under the catalog asset and a shape governs it.
    assert!(CLASSES.contains("cat:DashboardAsset a owl:Class"));
    assert!(CLASSES.contains("rdfs:subClassOf cat:Asset ."));
    assert!(SHAPES.contains("cat:DashboardAssetShape"));
}
