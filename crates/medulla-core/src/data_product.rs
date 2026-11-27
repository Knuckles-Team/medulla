//! Data contract checks (MEDULLA-TYPED-PACKS-R805.2).
//!
//! One pure function compares a product's contract with live catalog,
//! quality and change-capture state and returns every broken promise.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_source_types::asset_usage::AssetUsage;
use medulla_catalog_source_types::quality::QualityScore;
use {
    medulla_catalog_source_types::data_product::ContractPromise,
    medulla_catalog_source_types::data_product::ContractViolation,
    medulla_catalog_source_types::data_product::DataContract,
    medulla_catalog_source_types::data_product::DataProduct,
    medulla_catalog_source_types::data_product::ProductProposal,
};

/// Live state a contract is checked against.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveContractState {
    /// Current catalog schema digest, `sha256:<64 lowercase hex>`.
    pub schema_digest: String,
    /// The latest score per quality rule.
    pub quality: Vec<QualityScore>,
    /// Age of the change-capture watermark, in seconds.
    pub watermark_age_s: u64,
    /// When the check ran, in epoch milliseconds.
    pub at_ms: u64,
}

/// Every promise of `contract` that `live` breaks, in schema, quality, freshness order.
pub fn check_contract(
    product: &DataProduct,
    contract: &DataContract,
    live: &LiveContractState,
) -> Vec<ContractViolation> {
    let violation = |promise, observed: String| ContractViolation {
        product: product.id.clone(),
        promise,
        observed,
        at_ms: live.at_ms,
    };
    let mut out = Vec::new();
    if live.schema_digest != contract.schema_digest {
        out.push(violation(
            ContractPromise::Schema,
            live.schema_digest.clone(),
        ));
    }
    // The first failing score of each rule, found once, so the check is linear in the rules and
    // the scores rather than their product.
    let mut failing: std::collections::HashMap<&str, &QualityScore> =
        std::collections::HashMap::new();
    for score in live.quality.iter().filter(|score| !score.passed) {
        failing.entry(score.rule.as_str()).or_insert(score);
    }
    for rule in &contract.quality_rules {
        if let Some(score) = failing.get(rule.as_str()) {
            out.push(violation(
                ContractPromise::Quality { rule: rule.clone() },
                score.value.to_string(),
            ));
        }
    }
    if live.watermark_age_s > contract.freshness_s {
        out.push(violation(
            ContractPromise::Freshness,
            live.watermark_age_s.to_string(),
        ));
    }
    out
}

/// Data products the usage suggests (MEDULLA-TYPED-PACKS-R805.7).
///
/// Within one period, assets form a cluster when at least `min_principals` principals each
/// queried every asset of it and it holds two or more assets. Clusters are built greedily over
/// the sorted asset names: an asset joins the open cluster while the principals that queried all
/// its members still number `min_principals`. An asset sits in at most one cluster per period.
/// The owner candidate is the common principal with the most queries over the cluster, the
/// lowest principal id on a tie. `terms` maps an asset to the glossary terms linked to it.
pub fn propose_products(
    usage: &[AssetUsage],
    terms: &BTreeMap<String, Vec<String>>,
    min_principals: usize,
) -> Vec<ProductProposal> {
    let min_principals = min_principals.max(1);
    // period -> asset -> principal -> queries
    let mut by_period: BTreeMap<&str, BTreeMap<&str, BTreeMap<&str, u64>>> = BTreeMap::new();
    for point in usage.iter().filter(|point| point.queries > 0) {
        *by_period
            .entry(point.period.as_str())
            .or_default()
            .entry(point.asset.as_str())
            .or_default()
            .entry(point.principal.as_str())
            .or_default() += point.queries;
    }
    let mut out = Vec::new();
    for (period, assets) in &by_period {
        let names: Vec<&str> = assets.keys().copied().collect();
        let mut used: BTreeSet<&str> = BTreeSet::new();
        for (index, seed) in names.iter().enumerate() {
            if used.contains(seed) {
                continue;
            }
            let mut common: BTreeSet<&str> = assets[seed].keys().copied().collect();
            if common.len() < min_principals {
                continue;
            }
            let mut members = vec![*seed];
            for next in &names[index + 1..] {
                if used.contains(next) {
                    continue;
                }
                let both: BTreeSet<&str> = common
                    .iter()
                    .copied()
                    .filter(|principal| assets[next].contains_key(principal))
                    .collect();
                if both.len() >= min_principals {
                    common = both;
                    members.push(*next);
                }
            }
            if members.len() < 2 {
                continue;
            }
            let queries = |principal: &str| -> u64 {
                members.iter().map(|asset| assets[asset][principal]).sum()
            };
            let owner = common
                .iter()
                .copied()
                .max_by(|a, b| queries(a).cmp(&queries(b)).then_with(|| b.cmp(a)))
                .map(str::to_string)
                .unwrap_or_default();
            let linked: BTreeSet<String> = members
                .iter()
                .filter_map(|asset| terms.get(*asset))
                .flatten()
                .cloned()
                .collect();
            used.extend(members.iter().copied());
            out.push(ProductProposal {
                datasets: members.iter().map(|asset| asset.to_string()).collect(),
                terms: linked.into_iter().collect(),
                owner_candidate: owner,
                principals: common.iter().map(|p| p.to_string()).collect(),
                period: period.to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use {
        medulla_catalog_source_types::data_product::ProductPort,
        medulla_catalog_source_types::data_product::ProductPortKind,
    };

    // spec: MEDULLA-TYPED-PACKS-R805.6
    #[test]
    fn contract_check_is_linear_in_promises() {
        const RULES: usize = 20_000;
        let rules: Vec<String> = (0..RULES).map(|n| format!("rule:{n}")).collect();
        let digest = format!("sha256:{}", "ab".repeat(32));
        let product = DataProduct {
            id: "product:linear".into(),
            owner: "team:data".into(),
            version: "1.0.0".into(),
            datasets: vec!["table:linear".into()],
            terms: vec![],
            ports: vec![ProductPort {
                name: "sql".into(),
                kind: ProductPortKind::Query,
            }],
        };
        let contract = DataContract {
            product: product.id.clone(),
            schema_digest: digest.clone(),
            quality_rules: rules.clone(),
            freshness_s: 60,
        };
        let live = LiveContractState {
            schema_digest: digest,
            quality: rules
                .iter()
                .map(|rule| QualityScore {
                    rule: rule.clone(),
                    value: 0.0,
                    passed: false,
                    at: 1,
                    query_digest: "ab".repeat(32),
                    rows_checked: 1,
                    rows_failed: 1,
                    checker: "native".into(),
                })
                .collect(),
            watermark_age_s: 0,
            at_ms: 1,
        };
        let started = std::time::Instant::now();
        let violations = check_contract(&product, &contract, &live);
        let elapsed = started.elapsed();
        // One violation per broken promise, in contract order.
        assert_eq!(violations.len(), RULES);
        assert_eq!(
            violations[RULES - 1].promise,
            ContractPromise::Quality {
                rule: "rule:19999".into()
            }
        );
        // A product of the rules and the scores (4e8 steps) would not finish inside this bound.
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?}"
        );
    }
}
