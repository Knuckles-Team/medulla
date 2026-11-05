//! Data contract checks (MEDULLA-TYPED-PACKS-R805.2).
//!
//! One pure function compares a product's contract with live catalog,
//! quality and change-capture state and returns every broken promise.

use medulla_types::data_product::{ContractPromise, ContractViolation, DataContract, DataProduct};
use medulla_types::quality::QualityScore;

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
