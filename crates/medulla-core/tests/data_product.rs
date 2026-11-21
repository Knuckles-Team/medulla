//! Contract check behavior (MEDULLA-TYPED-PACKS-R805.2).

use std::collections::BTreeMap;

use medulla_catalog_types::asset_usage::AssetUsage;
use medulla_catalog_types::data_product::{
    ContractPromise, DataContract, DataProduct, ProductPort, ProductPortKind,
};
use medulla_catalog_types::quality::QualityScore;
use medulla_core::data_product::{check_contract, LiveContractState};

// spec: MEDULLA-TYPED-PACKS-R805.2
#[test]
fn breach_records_violation() {
    let product = DataProduct {
        id: "product:orders".into(),
        owner: "team:sales".into(),
        version: "1.0.0".into(),
        datasets: vec!["table:orders".into()],
        terms: vec![],
        ports: vec![ProductPort {
            name: "sql".into(),
            kind: ProductPortKind::Query,
        }],
    };
    let digest = format!("sha256:{}", "ab".repeat(32));
    let contract = DataContract {
        product: product.id.clone(),
        schema_digest: digest.clone(),
        quality_rules: vec!["rule:not-null".into()],
        freshness_s: 3_600,
    };
    let mut live = LiveContractState {
        schema_digest: digest,
        quality: vec![QualityScore {
            rule: "rule:not-null".into(),
            value: 1.0,
            passed: true,
            at: 1,
        }],
        watermark_age_s: 60,
        at_ms: 42,
    };
    assert!(check_contract(&product, &contract, &live).is_empty());
    live.watermark_age_s = 7_200;
    let violations = check_contract(&product, &contract, &live);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].promise, ContractPromise::Freshness);
    assert_eq!(violations[0].observed, "7200");
    assert_eq!(violations[0].at_ms, 42);
}

fn point(asset: &str, principal: &str, period: &str, queries: u64) -> AssetUsage {
    AssetUsage {
        asset: asset.into(),
        principal: principal.into(),
        period: period.into(),
        reads: queries,
        queries,
    }
}

// spec: MEDULLA-TYPED-PACKS-R805.7
#[test]
fn proposals_cluster_by_principal_overlap() {
    let mut usage = Vec::new();
    // Three principals each query orders and customers; "bob" and "amy" tie at 4, "zed" has 2.
    for (principal, count) in [("amy", 4), ("bob", 4), ("zed", 2)] {
        usage.push(point("orders", principal, "p1", count / 2));
        usage.push(point("customers", principal, "p1", count - count / 2));
    }
    // Two principals only touch invoices: below the minimum of three.
    usage.push(point("invoices", "amy", "p1", 9));
    usage.push(point("invoices", "bob", "p1", 9));
    // The same pair in another period has only two principals.
    usage.push(point("orders", "amy", "p2", 1));
    usage.push(point("customers", "amy", "p2", 1));
    usage.push(point("orders", "bob", "p2", 1));
    usage.push(point("customers", "bob", "p2", 1));
    let mut terms = BTreeMap::new();
    terms.insert("orders".to_string(), vec!["term:sale".to_string()]);
    terms.insert(
        "customers".to_string(),
        vec!["term:party".to_string(), "term:sale".to_string()],
    );

    let found = propose_products(&usage, &terms, 3);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].datasets, vec!["customers", "orders"]);
    assert_eq!(found[0].terms, vec!["term:party", "term:sale"]);
    assert_eq!(found[0].principals, vec!["amy", "bob", "zed"]);
    assert_eq!(found[0].owner_candidate, "amy", "tie goes to the lowest id");
    assert_eq!(found[0].period, "p1");

    // With a minimum of two, the second period and the invoice pair also qualify.
    assert!(propose_products(&usage, &terms, 2).len() >= 2);
    // No usage, no proposal.
    assert!(propose_products(&[], &terms, 3).is_empty());
}
