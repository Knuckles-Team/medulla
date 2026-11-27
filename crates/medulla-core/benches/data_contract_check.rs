//! Data contract check (MEDULLA-TYPED-PACKS-R805.6).
//!
//! Nightly bench group `decide/data_contract_check`: `check_contract` over 1000 contracts.
//! Budget per contract: p99 below 1 ms. The run exits non-zero when the budget is exceeded.
//! A service level, never a pull request gate; the gate is the served access request journey.
//!
// spec: MEDULLA-TYPED-PACKS-R805.6
//!
//! Run: cargo bench -p medulla-core --bench data_contract_check

use std::time::{Duration, Instant};

use medulla_catalog_types::data_product::{
    DataContract, DataProduct, ProductPort, ProductPortKind,
};
use medulla_catalog_types::quality::QualityScore;
use medulla_core::data_product::{check_contract, LiveContractState};

const CONTRACTS: usize = 1000;
const RULES: usize = 8;
const BUDGET_P99: Duration = Duration::from_millis(1);

fn main() {
    let digest = format!("sha256:{}", "ab".repeat(32));
    let rules: Vec<String> = (0..RULES).map(|n| format!("rule:bench-{n}")).collect();
    let live = LiveContractState {
        schema_digest: format!("sha256:{}", "cd".repeat(32)),
        quality: rules
            .iter()
            .map(|rule| QualityScore {
                rule: rule.clone(),
                value: 0.0,
                passed: false,
                at: 1,
                query_digest: "ab".repeat(32),
                rows_checked: 10,
                rows_failed: 10,
                checker: "bench".to_string(),
            })
            .collect(),
        watermark_age_s: 7_200,
        at_ms: 1,
    };
    let pairs: Vec<(DataProduct, DataContract)> = (0..CONTRACTS)
        .map(|n| {
            (
                DataProduct {
                    id: format!("product:bench-{n}"),
                    owner: "team:bench".to_string(),
                    version: "1.0.0".to_string(),
                    datasets: vec![format!("table:bench-{n}")],
                    terms: Vec::new(),
                    ports: vec![ProductPort {
                        name: "sql".to_string(),
                        kind: ProductPortKind::Query,
                    }],
                },
                DataContract {
                    product: format!("product:bench-{n}"),
                    schema_digest: digest.clone(),
                    quality_rules: rules.clone(),
                    freshness_s: 3_600,
                },
            )
        })
        .collect();
    let mut samples = Vec::with_capacity(CONTRACTS);
    for (product, contract) in &pairs {
        let started = Instant::now();
        let violations = check_contract(product, contract, &live);
        samples.push(started.elapsed());
        assert_eq!(violations.len(), RULES + 2);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("decide/data_contract_check p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
