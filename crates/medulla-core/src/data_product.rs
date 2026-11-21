//! Data contract checks (MEDULLA-TYPED-PACKS-R805.2).
//!
//! One pure function compares a product's contract with live catalog,
//! quality and change-capture state and returns every broken promise.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::asset_usage::AssetUsage;
use medulla_catalog_types::data_product::{
    ContractPromise, ContractViolation, DataContract, DataProduct, ProductProposal,
};
use medulla_catalog_types::quality::QualityScore;

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
    for rule in &contract.quality_rules {
        if let Some(score) = live.quality.iter().find(|s| &s.rule == rule && !s.passed) {
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
