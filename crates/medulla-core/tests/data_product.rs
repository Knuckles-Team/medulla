//! Contract check behavior (MEDULLA-TYPED-PACKS-R805.2).

use medulla_core::data_product::{LiveContractState, check_contract};
use medulla_types::data_product::{
    ContractPromise, DataContract, DataProduct, ProductPort, ProductPortKind,
};
use medulla_types::quality::QualityScore;

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
